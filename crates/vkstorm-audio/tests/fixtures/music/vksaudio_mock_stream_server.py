#!/usr/bin/env python3
"""\
@file   vksaudio_mock_stream_server.py
@brief  Scripted local internet-radio server for the parcel music tests.

Serves the music fixtures (tones encoded as MP3, MP2, Ogg Vorbis (plain and
chained), AAC-LC, HE-AAC v1/v2, Opus, FLAC, Ogg FLAC) the way radio servers
do: finite files, live streams without a length, SHOUTcast's non-HTTP
"ICY 200 OK" status line, interleaved ICY metadata, playlists, an Icecast
status-json.xsl document, connections that drop, slow starts, real-time
pacing; and what must be refused: video, an audio-only MP4, a web page, HLS.
No external network is used.

Usage: vksaudio_mock_stream_server.py [--media DIR] [--log FILE]
                                      [--serve PORT] [--] EXE [ARGS...]

With --serve PORT it only serves (PORT 0 picks a free port) and prints
"serving on 127.0.0.1:<port>" on stdout once listening. Otherwise it picks a
free port, exports LL_TEST_PORT (and VKS_AUDIO_MOCK_PORT) to the child, runs
it and exits with the child's return code (2 when it cannot listen).
--media defaults to fixtures/music next to this script, else the script's
own directory.

Windows note: on the Windows 11 / Python 3.14 machine this was developed
on, a connection the Python server closes first loses its FIN about a
third of the time (the client sees a reset or waits for the end), while
connections the client closes are clean. Sized bodies (Content-Length)
therefore wait for the client to close; only live (close-delimited)
streams end with a server close, and tests that need a clean end use
length=1 or lengthafter=N.

Paths (every other path is a 404):
  /media/<file>        the whole file with Content-Length (a finite stream)
  /live/<file>         a live stream: no Content-Length, HTTP/1.0, closed at
                       the end (a disconnect, for the player)
  /playlist.pls?e=U&e=U  a PLS listing the entries (a U starting with / is
  /playlist.m3u?e=U...   made absolute with this server's address)
  /hls.m3u8            an HLS media playlist (refused by the player)
  /redirect?to=U       302 to U
  /status/<code>       an empty response with that HTTP status
  .../status-json.xsl  the Icecast status document when configured for that
                       exact path (see /config/status), else 404
  /config/status?at=P&listen=L&title=T[&artist=A]
                       serve status-json.xsl at path P, with two sources:
                       an unrelated one and one whose listenurl ends with L
  /log                 the requests so far as JSON (path, user agent,
                       Icy-MetaData header, time in ms since start)

Query options of /media and /live:
  ct=TYPE        Content-Type override
  slowstart=MS   wait before answering
  pace=P         realtime, or a factor of real time (e.g. 4), or none
                 (default: as fast as the client reads)
  burst=BYTES    sent at once before pacing (Icecast's connect burst)
  stall=MS@BYTES pause MS after BYTES of audio (repeatable)
/live only:
  loops=N        play the file N times (default 1)
  icy=1          interleave ICY metadata (icy-metaint) when the request
                 has "Icy-MetaData: 1"
  metaint=N      audio bytes between metadata blocks (default 8192)
  title=T        StreamTitle values (repeatable; cycled, a new one every
                 `every` blocks, default 4; a block repeats the current
                 title only with repeat=1, else it is empty)
  enc=latin1     send the titles in Latin-1 instead of UTF-8
  status=icy     answer "ICY 200 OK" (SHOUTcast 1) instead of HTTP/1.0
  length=1       send a Content-Length (a finite live-style stream)
  lengthafter=N  send a Content-Length from the N+1-th connection with the
                 same key on (e.g. a live stream that drops, then a
                 reconnect that plays to a clean end)
  drop=BYTES     close the connection abruptly after BYTES of audio ...
  drops=N        ... on the first N connections with the same key=K
                 (default 1); later connections play normally
  key=K          the counter for drop=, fail=, maxconn= and lengthafter=
  fail=N         the first N connections with key K get a 503
  maxconn=N      connections after the first N with key K get a 503

$LicenseInfo:firstyear=2026&license=viewerlgpl$
Vulkanstorm Viewer Source Code
Copyright (C) 2026, Vulkanstorm Viewer Project
Licensed under the GNU Lesser General Public License, version 2.1.
$/LicenseInfo$
"""

import json
import os
import socket
import struct
import subprocess
import sys
import threading
import time
import urllib.parse

CONTENT_TYPES = {
    ".mp3": "audio/mpeg",
    ".mp2": "audio/mpeg",
    ".ogg": "audio/ogg",
    ".oga": "audio/ogg",
    ".opus": "audio/ogg; codecs=opus",
    ".aac": "audio/aac",
    ".flac": "audio/flac",
    ".mp4": "video/mp4",
    ".m4a": "audio/mp4",
    ".ogv": "video/ogg",
    ".html": "text/html; charset=utf-8",
}


class State:
    def __init__(self, media):
        self.lock = threading.Lock()
        self.media = media
        self.t0 = time.perf_counter()
        self.counts = {}
        self.status_at = None
        self.status_doc = None
        self.log = []
        self.log_file = None
        try:
            with open(os.path.join(media, "manifest.json"), encoding="utf-8") as f:
                self.manifest = json.load(f)
        except OSError:
            self.manifest = {}

    def count(self, key):
        with self.lock:
            n = self.counts.get(key, 0)
            self.counts[key] = n + 1
            return n

    def record(self, entry):
        with self.lock:
            entry["t"] = round((time.perf_counter() - self.t0) * 1000.0, 1)
            self.log.append(entry)
            if self.log_file:
                self.log_file.write(json.dumps(entry) + "\n")
                self.log_file.flush()


def header(headers, name):
    name = name.lower()
    for n, v in headers:
        if n.lower() == name:
            return v
    return None


class Connection(threading.Thread):
    def __init__(self, sock, state, port):
        super().__init__(daemon=True)
        self.sock = sock
        self.state = state
        self.port = port
        self.abort = False
        # The body has a Content-Length: the client closes the connection.
        self.sized = False

    def read_head(self):
        buf = b""
        while b"\r\n\r\n" not in buf:
            data = self.sock.recv(65536)
            if not data:
                return None
            buf += data
            if len(buf) > 65536:
                return None
        head = buf[:buf.index(b"\r\n\r\n")].decode("latin-1").split("\r\n")
        method, target, _ = head[0].split(" ", 2)
        headers = []
        for line in head[1:]:
            name, _, value = line.partition(":")
            headers.append((name.strip(), value.strip()))
        return method, target, headers

    def run(self):
        req = None
        try:
            req = self.read_head()
            if req is not None:
                self.respond(*req)
        except ConnectionError:
            pass
        except (OSError, ValueError) as e:
            # Not a client going away: worth seeing in the test's output.
            sys.stderr.write("vksaudio_mock_stream_server: %s: %r%s" % (req[1] if req else "?", e, os.linesep))
        finally:
            try:
                if not self.abort:
                    # Let the client close first where it can (a sized
                    # body), else send FIN and wait for its close: a server
                    # that closes at once makes Windows Python reset some
                    # connections (seen on Windows 11 with Python 3.14:
                    # truncated bodies, resets before any data), which the
                    # tests would misread as stream failures.
                    if not self.sized:
                        self.sock.shutdown(socket.SHUT_WR)
                    self.sock.settimeout(10)
                    while self.sock.recv(65536):
                        pass
            except OSError:
                pass
            try:
                self.sock.close()
            except OSError:
                pass

    def send_head(self, status_line, headers):
        lines = [status_line] + ["%s: %s" % h for h in headers]
        self.sock.sendall(("\r\n".join(lines) + "\r\n\r\n").encode("latin-1"))

    def simple(self, code, reason, ctype, body, extra=()):
        self.sized = True
        self.send_head("HTTP/1.1 %d %s" % (code, reason),
                       [("Content-Type", ctype), ("Content-Length", str(len(body)))] + list(extra))
        self.sock.sendall(body)

    def base(self):
        return "http://127.0.0.1:%d" % self.port

    def respond(self, method, target, headers):
        path, _, query = target.partition("?")
        q = urllib.parse.parse_qs(query, keep_blank_values=True)
        one = lambda k, d=None: q.get(k, [d])[0]
        self.state.record(dict(path=path, query=query, ua=header(headers, "user-agent") or "",
                               icy=header(headers, "icy-metadata") or ""))
        if path == "/log":
            with self.state.lock:
                body = json.dumps(self.state.log).encode()
            return self.simple(200, "OK", "application/json", body)
        if path == "/config/status":
            listen = one("listen", "/")
            src = {"listenurl": self.base() + listen, "title": one("title", "")}
            if one("artist") is not None:
                src["artist"] = one("artist")
            doc = {"icestats": {"admin": "admin@localhost", "source": [
                {"listenurl": self.base() + "/elsewhere", "title": "Wrong - Source"}, src]}}
            with self.state.lock:
                self.state.status_at = one("at")
                self.state.status_doc = json.dumps(doc).encode()
            return self.simple(200, "OK", "text/plain", b"ok")
        if path.endswith("/status-json.xsl"):
            with self.state.lock:
                at, doc = self.state.status_at, self.state.status_doc
            if at == path and doc is not None:
                return self.simple(200, "OK", "application/json", doc)
            return self.simple(404, "Not Found", "text/plain", b"not found")
        if path.startswith("/status/"):
            code = int(path[len("/status/"):])
            return self.simple(code, "Status", "text/plain", b"")
        if path == "/redirect":
            return self.simple(302, "Found", "text/plain", b"", [("Location", one("to", "/"))])
        if path in ("/playlist.pls", "/playlist.m3u"):
            entries = [e if not e.startswith("/") else self.base() + e for e in q.get("e", [])]
            if path.endswith(".pls"):
                lines = ["[playlist]", "NumberOfEntries=%d" % len(entries)]
                for i, e in enumerate(entries, 1):
                    lines += ["File%d=%s" % (i, e), "Title%d=Entry %d" % (i, i), "Length%d=-1" % i]
                lines.append("Version=2")
                return self.simple(200, "OK", "audio/x-scpls", ("\n".join(lines) + "\n").encode())
            lines = ["#EXTM3U"]
            for i, e in enumerate(entries, 1):
                lines += ["#EXTINF:-1,Entry %d" % i, e]
            return self.simple(200, "OK", "audio/x-mpegurl", ("\r\n".join(lines) + "\r\n").encode())
        if path == "/hls.m3u8":
            body = b"#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:10\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:10.0,\nseg0.ts\n"
            return self.simple(200, "OK", "application/vnd.apple.mpegurl", body)
        for prefix, live in (("/media/", False), ("/live/", True)):
            if path.startswith(prefix):
                return self.media(path[len(prefix):], live, q, one, headers)
        return self.simple(404, "Not Found", "text/plain", b"not found")

    def media(self, name, live, q, one, headers):
        name = os.path.basename(urllib.parse.unquote(name))
        try:
            with open(os.path.join(self.state.media, name), "rb") as f:
                data = f.read()
        except OSError:
            return self.simple(404, "Not Found", "text/plain", b"not found")
        key = one("key", name)
        nconn = self.state.count(key)
        if nconn < int(one("fail", "0")) or nconn >= int(one("maxconn", "1000000")):
            return self.simple(503, "Service Unavailable", "text/plain", b"busy")
        slowstart = float(one("slowstart", "0")) / 1000.0
        if slowstart > 0:
            time.sleep(slowstart)
        ext = os.path.splitext(name)[1].lower()
        ctype = one("ct") or CONTENT_TYPES.get(ext, "application/octet-stream")
        loops = int(one("loops", "1")) if live else 1
        audio = data * loops
        icy = live and one("icy") == "1" and header(headers, "icy-metadata") == "1"
        metaint = int(one("metaint", "8192"))
        titles = q.get("title", [])
        every = max(1, int(one("every", "4")))
        repeat = one("repeat") == "1"
        enc = "latin-1" if one("enc") == "latin1" else "utf-8"
        drop = int(one("drop", "-1")) if nconn < int(one("drops", "1")) else -1

        # The body as a list of (audio bytes, metadata block) pieces, so
        # pacing counts audio only.
        pieces = []
        if icy:
            last = None
            for i, k in enumerate(range(0, len(audio), metaint)):
                chunk = audio[k:k + metaint]
                block = b"\x00"
                if len(chunk) == metaint:
                    title = titles[(i // every) % len(titles)] if titles else None
                    if title is not None and (title != last or repeat):
                        text = ("StreamTitle='%s';StreamUrl='';" % title).encode(enc)
                        text += b"\x00" * (-len(text) % 16)
                        block = bytes([len(text) // 16]) + text
                        last = title
                else:
                    block = b""
                pieces.append((chunk, block))
        else:
            pieces = [(audio, b"")]
        total = sum(len(a) + len(b) for a, b in pieces)

        if live and one("status") == "icy":
            status = "ICY 200 OK"
            hdrs = [("icy-name", "Mock Radio"), ("icy-genre", "Test"), ("icy-br", "128"),
                    ("content-type", ctype)]
        else:
            status = "HTTP/1.0 200 OK" if live else "HTTP/1.1 200 OK"
            hdrs = [("Content-Type", ctype), ("Cache-Control", "no-cache")]
            if live:
                hdrs += [("icy-name", "Mock Radio"), ("icy-br", "128")]
        if icy:
            hdrs.append(("icy-metaint", str(metaint)))
        if not live or one("length") == "1" or nconn >= int(one("lengthafter", "1000000")):
            hdrs.append(("Content-Length", str(total)))
            self.sized = True
        else:
            hdrs.append(("Connection", "close"))
        self.send_head(status, hdrs)

        # Pacing: audio bytes due by now = burst + rate * elapsed.
        pace = one("pace", "none")
        rate = None
        if pace != "none":
            info = self.state.manifest.get(name, {})
            seconds = float(info.get("seconds", 0)) or 1.0
            factor = 1.0 if pace == "realtime" else float(pace)
            rate = len(data) / seconds * factor
        burst = int(one("burst", "0"))
        stalls = []
        for s in q.get("stall", []):
            ms, _, at = s.partition("@")
            stalls.append((int(at), float(ms) / 1000.0))
        stalls.sort()
        t0 = time.perf_counter()
        paused = 0.0
        sent_audio = 0
        for chunk, block in pieces:
            pos = 0
            while pos < len(chunk):
                n = min(4096, len(chunk) - pos)
                if drop >= 0 and sent_audio + n > drop:
                    n = drop - sent_audio
                    if n > 0:
                        self.sock.sendall(chunk[pos:pos + n])
                    # Let the client read what was sent (a reset makes the
                    # receiver discard unread data), then close abruptly:
                    # RST instead of an orderly FIN.
                    time.sleep(0.5)
                    linger = struct.pack("HH" if os.name == "nt" else "ii", 1, 0)
                    self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, linger)
                    self.abort = True
                    return
                while stalls and sent_audio >= stalls[0][0]:
                    time.sleep(stalls[0][1])
                    paused += stalls[0][1]
                    stalls.pop(0)
                if rate is not None:
                    due = burst + rate * (time.perf_counter() - t0 - paused)
                    if sent_audio + n > due:
                        time.sleep(min(0.05, (sent_audio + n - due) / rate))
                        continue
                self.sock.sendall(chunk[pos:pos + n])
                pos += n
                sent_audio += n
            if block:
                self.sock.sendall(block)


def serve(port, state, stop):
    # The listening socket: a failure to bind or listen is reported to the
    # wrapper, which would otherwise wait for readiness forever.
    try:
        srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        srv.bind(("127.0.0.1", port))
        srv.listen(64)
        srv.settimeout(0.2)
        stop["port"] = srv.getsockname()[1]
    except OSError as e:
        stop["error"] = e
        stop["ready"].set()
        return
    stop["ready"].set()
    while not stop["flag"]:
        try:
            sock, _ = srv.accept()
            sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        except socket.timeout:
            continue
        except OSError:
            # A connection reset before it was accepted: keep serving.
            continue
        Connection(sock, state, stop["port"]).start()
    srv.close()


def main(argv):
    here = os.path.dirname(os.path.abspath(__file__))
    media = os.path.join(here, "fixtures", "music")
    if not os.path.isdir(media):
        media = here
    log = None
    port = 0
    serve_only = False
    i = 0
    while i < len(argv):
        if argv[i] == "--media":
            media = argv[i + 1]
            i += 2
        elif argv[i] == "--log":
            log = argv[i + 1]
            i += 2
        elif argv[i] == "--serve":
            port = int(argv[i + 1])
            serve_only = True
            i += 2
        elif argv[i] == "--":
            i += 1
            break
        else:
            break
    cmd = argv[i:]
    if not serve_only and not cmd:
        sys.stderr.write(__doc__)
        return 2
    state = State(media)
    if log:
        state.log_file = open(log, "a", encoding="utf-8")
    stop = dict(flag=False, ready=threading.Event(), port=None, error=None)
    thread = threading.Thread(target=serve, args=(port, state, stop), daemon=True)
    thread.start()
    if not stop["ready"].wait(30) or stop["error"] is not None:
        sys.stderr.write("vksaudio_mock_stream_server: cannot listen on 127.0.0.1:%d: %s\n"
                         % (port, stop["error"] or "no readiness within 30 s"))
        return 2
    if serve_only:
        print("serving on 127.0.0.1:%d" % stop["port"], flush=True)
        try:
            # Ends when stdin closes (the test process went away) or on ^C.
            sys.stdin.read()
        except (KeyboardInterrupt, OSError, ValueError):
            pass
        return 0
    env = dict(os.environ)
    env["LL_TEST_PORT"] = str(stop["port"])
    env["VKS_AUDIO_MOCK_PORT"] = str(stop["port"])
    rc = subprocess.call(cmd, env=env)
    stop["flag"] = True
    thread.join()
    if state.log_file is not None:
        state.log_file.close()
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
