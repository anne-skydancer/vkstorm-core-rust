"""Writes the input side of the AIS update corpus (recorder R3a, survey 3 layer A).

usage: python gen_cases.py <cases directory>

Each case is a scripted skeleton (the inventory model the response meets) and
one or more steps. A step is one AIS command result exactly as
AISAPI::InvokeAISCommandCoro hands it to AISUpdate(result, type, body): the
LLSD map the HTTP adapter produced from the response, http_result included.
The C++ test (indra/newview/tests/llaisupdate_parity_test.cpp in the viewer)
reads these files, runs the real llaisapi.cpp on them and records what it does.

Files written:
  index.txt              the case ids in run order (the order is part of the
                         record: see the README)
  <id>.case.xml          LLSD XML: id, set, title, agent_id, session_id,
                         root_id, sched {call_us, frame_us}, large, seed,
                         skeleton {categories, items}, steps [{type,
                         target_id, url, body, response, same_frame,
                         disconnected, callback}]
  <id>.<n>.xml[.gz]      the result of step n, raw LLSD XML (set 11 holds
                         duplicate map keys on purpose; set 10 is gzipped)

Everything is deterministic: patterned UUIDs (see ids below), a seeded
xorshift64* for the large case, gzip with mtime 0.
"""
import gzip
import io
import os
import sys

# ---------------------------------------------------------------------------
# LLSD XML writer
# ---------------------------------------------------------------------------


class U(str):
    """An LLSD uuid."""


class Undef:
    """The LLSD undefined value."""


class Dup(list):
    """An LLSD map written with its keys in this order, duplicates allowed:
    a list of (key, value) pairs."""


class Raw(str):
    """Text written verbatim (already XML)."""


NULL = U("00000000-0000-0000-0000-000000000000")


def _esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def _xml(v, out, ind, step):
    pad = " " * ind if step else ""
    nl = "\n" if step else ""
    if isinstance(v, Raw):
        out.append(pad + v + nl)
    elif v is None or isinstance(v, Undef):
        out.append(pad + "<undef />" + nl)
    elif isinstance(v, bool):
        out.append(pad + "<boolean>" + ("true" if v else "false") + "</boolean>" + nl)
    elif isinstance(v, int):
        if not -2147483648 <= v <= 2147483647:
            raise ValueError("LLSD integer out of range: %d" % v)
        out.append(pad + "<integer>%d</integer>" % v + nl)
    elif isinstance(v, U):
        out.append(pad + "<uuid>" + v + "</uuid>" + nl)
    elif isinstance(v, str):
        out.append(pad + "<string>" + _esc(v) + "</string>" + nl)
    elif isinstance(v, Dup) or isinstance(v, dict):
        pairs = list(v) if isinstance(v, Dup) else list(v.items())
        if not pairs:
            out.append(pad + "<map />" + nl)
            return
        out.append(pad + "<map>" + nl)
        for k, x in pairs:
            out.append(" " * (ind + step) + "<key>" + _esc(k) + "</key>" + nl)
            _xml(x, out, ind + step, step)
        out.append(pad + "</map>" + nl)
    elif isinstance(v, list):
        if not v:
            out.append(pad + "<array />" + nl)
            return
        out.append(pad + "<array>" + nl)
        for x in v:
            _xml(x, out, ind + step, step)
        out.append(pad + "</array>" + nl)
    else:
        raise TypeError("cannot write %r" % (v,))


def llsd_xml(v, compact=False):
    out = ['<?xml version="1.0" ?>\n<llsd>\n']
    _xml(v, out, 0 if compact else 1, 0 if compact else 1)
    if compact:
        out.append("\n")
    out.append("</llsd>\n")
    return "".join(out)


# ---------------------------------------------------------------------------
# Patterned ids. LLUUID orders by bytes, i.e. by the hex text, so the order in
# which AISUpdate's std::maps apply objects is the order of these strings:
# items (a...) < links (b...) < categories (c...).
# ---------------------------------------------------------------------------


def C(n):
    return U("c0000000-0000-4000-8000-%012x" % n)


def I(n):
    return U("a0000000-0000-4000-8000-%012x" % n)


def L(n):
    return U("b0000000-0000-4000-8000-%012x" % n)


def E(n):
    return U("e0000000-0000-4000-8000-%012x" % n)


AGENT = U("0a000000-0000-4000-8000-000000000001")
SESSION = U("0b000000-0000-4000-8000-000000000001")
OTHER = U("0a000000-0000-4000-8000-0000000000ff")
ROOT = C(0x1)
CLOTHING = C(0x5)
OBJECTS = C(0x6)
BODYPARTS = C(0xD)
TRASH = C(0xE)
LAF = C(0x10)
COF = C(0x2E)
MYOUTFITS = C(0x30)
TEXTURES = C(0x31)

# Folder types (llfoldertype.h), asset types (llassettype.h), inventory
# types (llinventorytype.h): raw integers, as AIS sends them.
FT_NONE, FT_TEXTURE, FT_CLOTHING, FT_OBJECT, FT_ROOT = -1, 0, 5, 6, 8
FT_BODYPART, FT_TRASH, FT_LAF, FT_COF, FT_OUTFIT, FT_MY_OUTFITS = 13, 14, 16, 46, 47, 48
AT_TEXTURE, AT_LANDMARK, AT_CLOTHING, AT_OBJECT, AT_NOTECARD = 0, 3, 5, 6, 7
AT_LSL, AT_BODYPART, AT_ANIMATION, AT_GESTURE, AT_LINK, AT_LINK_FOLDER = 10, 13, 20, 21, 24, 25
AT_MESH, AT_SETTINGS, AT_GLTF, AT_GLTF_BIN = 49, 56, 58, 59
IT_TEXTURE, IT_LANDMARK, IT_OBJECT, IT_NOTECARD, IT_CATEGORY, IT_LSL = 0, 3, 6, 7, 8, 10
IT_WEARABLE, IT_ANIMATION, IT_GESTURE, IT_MESH, IT_SETTINGS, IT_GLTF, IT_GLTF_BIN = 18, 19, 20, 22, 25, 27, 28

PERM_ALL = 0x7FFFFFFF
PERM_NEXT_DEFAULT = 0x82000  # PERM_MOVE | PERM_TRANSFER

CAP = "https://inventory.grid.example/cap/00000000-0000-4000-8000-00000000cafe"
LIBCAP = "https://library.grid.example/cap/00000000-0000-4000-8000-00000000beef"


def perms(owner=AGENT, creator=AGENT, base=PERM_ALL, owner_mask=PERM_ALL, group_mask=0,
          everyone=0, next_mask=PERM_NEXT_DEFAULT, group=NULL, last_owner=None):
    return {
        "creator_id": creator,
        "owner_id": owner,
        "last_owner_id": last_owner if last_owner is not None else owner,
        "group_id": group,
        "is_owner_group": False,
        "base_mask": base,
        "owner_mask": owner_mask,
        "group_mask": group_mask,
        "everyone_mask": everyone,
        "next_owner_mask": next_mask,
    }


def asset_for(item_id, salt=0):
    return U("e" + item_id[1:-4] + "%04x" % ((int(item_id[-4:], 16) + salt) & 0xFFFF))


def item(iid, parent, name, atype=AT_TEXTURE, itype=IT_TEXTURE, asset=None, flags=0, desc="",
         created=1700000000, sale=(0, 0), perm=None, extra=None):
    m = {
        "item_id": iid,
        "parent_id": parent,
        "asset_id": asset if asset is not None else asset_for(iid),
        "type": atype,
        "inv_type": itype,
        "flags": flags,
        "name": name,
        "desc": desc,
        "created_at": created,
        "permissions": perm if perm is not None else perms(),
        "sale_info": {"sale_type": sale[0], "sale_price": sale[1]},
        "agent_id": AGENT,
    }
    if extra:
        m.update(extra)
    return m


def link(lid, parent, target, name, atype=AT_LINK, itype=IT_WEARABLE, embedded=None, desc="",
         created=1700000500):
    m = {
        "item_id": lid,
        "parent_id": parent,
        "linked_id": target,
        "type": atype,
        "inv_type": itype,
        "name": name,
        "desc": desc,
        "created_at": created,
        "agent_id": AGENT,
    }
    if embedded is not None:
        m["_embedded"] = embedded
    return m


def cat(cid, parent, name, ftype=FT_NONE, version=None, embedded=None, extra=None, agent=True):
    m = {"category_id": cid, "parent_id": parent, "name": name, "type_default": ftype}
    if agent:
        m["agent_id"] = AGENT
    if version is not None:
        m["version"] = version
    if extra:
        m.update(extra)
    if embedded is not None:
        m["_embedded"] = embedded
    return m


def emb(categories=None, links=None, items=None, single_item=None, single_category=None):
    """An _embedded map; a key is present only when given (an empty dict is
    an empty but present map)."""
    m = {}
    if categories is not None:
        m["categories"] = {x["category_id"]: x for x in categories} if isinstance(categories, list) else categories
    if links is not None:
        m["links"] = {x["item_id"]: x for x in links} if isinstance(links, list) else links
    if items is not None:
        m["items"] = {x["item_id"]: x for x in items} if isinstance(items, list) else items
    if single_item is not None:
        m["item"] = single_item
    if single_category is not None:
        m["category"] = single_category
    return m


# ---------------------------------------------------------------------------
# http_result as LLCoreHttpUtil's HttpCoroHandler writes it (llcorehttputil.cpp
# writeStatusCodes / buildStatusEntry / onCompleted). The bodies of failures
# go to http_result.error_body; a 4xx map body becomes the result map, a 4xx
# non-map body goes under "content"; 5xx and transport failures leave {}.
# ---------------------------------------------------------------------------


def http_ok(url):
    return {"success": True, "type": 200, "status": 0, "message": "", "url": url,
            "headers": {"content-type": "application/llsd+xml"}}


def http_fail(url, code, error_body="", message="", status=1):
    return {"success": False, "type": code, "status": status, "message": message, "url": url,
            "headers": {"content-type": "application/llsd+xml"}, "error_body": error_body}


def result(body, http):
    if isinstance(body, Dup):
        out = Dup(body)
        out.append(("http_result", http))
        return out
    out = dict(body)
    out["http_result"] = http
    return out


def url_children(cid, depth, cap=CAP):
    return "%s/category/%s/children?depth=%d" % (cap, cid, depth)


def url_item(iid, cap=CAP):
    return "%s/item/%s" % (cap, iid)


def url_cat(cid, cap=CAP):
    return "%s/category/%s" % (cap, cid)


# ---------------------------------------------------------------------------
# Skeleton
# ---------------------------------------------------------------------------


class Skel:
    def __init__(self, base=True):
        self.cats = []
        self.items = []
        self._counts = {}
        if base:
            self.cat(ROOT, NULL, "My Inventory", FT_ROOT, 50)
            self.cat(CLOTHING, ROOT, "Clothing", FT_CLOTHING, 7)
            self.cat(OBJECTS, ROOT, "Objects", FT_OBJECT, 3)
            self.cat(BODYPARTS, ROOT, "Body Parts", FT_BODYPART, 4)
            self.cat(TRASH, ROOT, "Trash", FT_TRASH, 9)
            self.cat(LAF, ROOT, "Lost And Found", FT_LAF, 2)
            self.cat(COF, ROOT, "Current Outfit", FT_COF, 10)
            self.cat(MYOUTFITS, ROOT, "My Outfits", FT_MY_OUTFITS, 6)
            self.cat(TEXTURES, ROOT, "Textures", FT_TEXTURE, 5)

    def cat(self, cid, parent, name, ftype=FT_NONE, version=-1, descendents=None, owner=AGENT):
        """descendents=None: the number of children the skeleton gives it
        (what the viewer would count), so accounting applies."""
        self.cats = [c for c in self.cats if c["category_id"] != cid]
        self.cats.append({"category_id": cid, "parent_id": parent, "name": name, "type_default": ftype,
                          "owner_id": owner, "version": version, "descendents": descendents})
        return cid

    def drop(self, cid):
        self.cats = [c for c in self.cats if c["category_id"] != cid]

    def item(self, m, complete=True):
        self.items = [x for x in self.items if x["item_id"] != m["item_id"]]
        m = dict(m)
        if not complete:
            m["_complete"] = False
        self.items.append(m)
        return m["item_id"]

    def to_llsd(self):
        children = {}
        for c in self.cats:
            children[c["parent_id"]] = children.get(c["parent_id"], 0) + 1
        for x in self.items:
            children[x["parent_id"]] = children.get(x["parent_id"], 0) + 1
        cats = []
        for c in self.cats:
            c = dict(c)
            if c["descendents"] is None:
                c["descendents"] = children.get(c["category_id"], 0)
            cats.append(c)
        return {"categories": cats, "items": list(self.items)}


# ---------------------------------------------------------------------------
# Cases
# ---------------------------------------------------------------------------

CASES = []


class Case:
    def __init__(self, cid, set_no, title, skel=None, call_us=100, frame_us=16667, large=False, seed=None,
                 others_us=None):
        self.id = cid
        self.set = set_no
        self.title = title
        self.skel = skel if skel is not None else Skel()
        self.call_us = call_us
        self.frame_us = frame_us
        self.others_us = others_us or []
        self.large = large
        self.seed = seed
        self.steps = []
        CASES.append(self)

    def step(self, ctype, res, target=NULL, url="", body=Undef(), same_frame=False, disconnected=False,
             callback=True, compact=False, gz=False):
        self.steps.append({"type": ctype, "target_id": target, "url": url, "body": body, "res": res,
                           "same_frame": same_frame, "disconnected": disconnected, "callback": callback,
                           "compact": compact, "gz": gz})
        return self

    def write(self, outdir):
        steps = []
        for n, s in enumerate(self.steps):
            name = "%s.%d.xml" % (self.id, n)
            text = s["res"] if isinstance(s["res"], Raw) else llsd_xml(s["res"], compact=s["compact"])
            data = text.encode("utf-8")
            if s["gz"]:
                name += ".gz"
                buf = io.BytesIO()
                with gzip.GzipFile(filename="", mode="wb", fileobj=buf, mtime=0, compresslevel=9) as g:
                    g.write(data)
                data = buf.getvalue()
            with open(os.path.join(outdir, name), "wb") as f:
                f.write(data)
            steps.append({"type": s["type"], "target_id": s["target_id"], "url": s["url"], "body": s["body"],
                          "response": name, "same_frame": s["same_frame"], "disconnected": s["disconnected"],
                          "callback": s["callback"]})
        d = {
            "id": self.id,
            "set": self.set,
            "title": self.title,
            "agent_id": AGENT,
            "session_id": SESSION,
            "root_id": ROOT,
            "sched": {"call_us": self.call_us, "frame_us": self.frame_us, "others_us": self.others_us},
            "large": self.large,
            "skeleton": self.skel.to_llsd(),
            "steps": steps,
        }
        if self.seed is not None:
            d["seed"] = "0x%016x" % self.seed
        with open(os.path.join(outdir, self.id + ".case.xml"), "wb") as f:
            f.write(llsd_xml(d).encode("utf-8"))


def depth_body(d):
    return {"depth": d}


# ---- set 1: fetch children at depth 0 ---------------------------------------


def set1():
    FA = C(0x200)

    def skel_a():
        s = Skel()
        s.cat(FA, ROOT, "Folder A")  # version and count unknown
        s.item(item(I(0x201), FA, "Existing texture"))
        return s

    def full_embedded(with_links=True):
        sub1 = cat(C(0x210), FA, "Sub one", version=3)
        sub2 = cat(C(0x211), FA, "Sub two", version=4, embedded=emb(categories={}, links={}, items={}))
        i1 = item(I(0x201), FA, "Existing texture renamed")
        i2 = item(I(0x202), FA, "New notecard", AT_NOTECARD, IT_NOTECARD)
        i3 = item(I(0x203), FA, "New object", AT_OBJECT, IT_OBJECT, flags=0x100)
        l1 = link(L(0x201), FA, I(0x201), "Link to texture", itype=IT_TEXTURE,
                  embedded=emb(single_item=item(I(0x201), FA, "Existing texture via link")))
        return emb(categories=[sub1, sub2], links=[l1] if with_links else None, items=[i1, i2, i3])

    c = Case("s01a_children_depth0_full", 1, "FETCHCATEGORYCHILDREN depth 0: categories, links and items "
             "present, count known (6), version set; subfolders at depth -1 get rows, no version", skel_a())
    url = url_children(FA, 0)
    c.step("FETCHCATEGORYCHILDREN", result(cat(FA, ROOT, "Folder A", version=7, embedded=full_embedded()), http_ok(url)),
           target=FA, url=url, body=depth_body(0))

    c = Case("s01b_children_depth0_no_links_key", 1, "FETCHCATEGORYCHILDREN depth 0 without the links key: "
             "count unknown, neither count nor version set", skel_a())
    c.step("FETCHCATEGORYCHILDREN", result(cat(FA, ROOT, "Folder A", version=7, embedded=full_embedded(False)),
                                           http_ok(url)), target=FA, url=url, body=depth_body(0))

    # COF: links only, with their targets embedded.
    s = Skel()
    s.cat(COF, ROOT, "Current Outfit", FT_COF, -1, -1)
    shirt = item(I(0x301), CLOTHING, "Shirt", AT_CLOTHING, IT_WEARABLE, flags=4)
    shape = item(I(0x302), BODYPARTS, "Shape", AT_BODYPART, IT_WEARABLE, flags=0)
    hat = item(I(0x303), OBJECTS, "Hat", AT_OBJECT, IT_OBJECT, flags=0)
    s.item(shape)
    links = [link(L(0x301), COF, I(0x301), "Shirt", embedded=emb(single_item=shirt)),
             link(L(0x302), COF, I(0x302), "Shape", embedded=emb(single_item=shape)),
             link(L(0x303), COF, I(0x303), "Hat", itype=IT_OBJECT, embedded=emb(single_item=hat))]
    c = Case("s01c_cof_links_only", 1, "FETCHCOF: COF holds links only, count = links, version set; link "
             "permissions forced; embedded targets parsed", s)
    url = CAP + "/category/current/links"
    c.step("FETCHCOF", result(cat(COF, ROOT, "Current Outfit", FT_COF, version=12, embedded=emb(links=links)),
                              http_ok(url)), url=url, body=depth_body(0))

    # An outfit folder fetched by its links.
    s = Skel()
    OUT = s.cat(C(0x320), MYOUTFITS, "Outfit one", FT_OUTFIT, -1, -1)
    s.item(shirt)
    c = Case("s01d_outfit_links_only", 1, "FETCHCATEGORYLINKS on an outfit folder: links only, count and "
             "version set", s)
    url = "%s/category/%s/links" % (CAP, OUT)
    c.step("FETCHCATEGORYLINKS", result(cat(OUT, MYOUTFITS, "Outfit one", FT_OUTFIT, version=9,
                                            embedded=emb(links=[link(L(0x321), OUT, I(0x301), "Shirt")])),
                                        http_ok(url)), url=url, body=depth_body(0))

    # A plain folder fetched by its links: not COF/outfit, so the count stays unknown.
    s = Skel()
    PL = s.cat(C(0x330), ROOT, "Plain", FT_NONE, -1, -1)
    s.item(shirt)
    c = Case("s01e_plain_folder_links_only", 1, "FETCHCATEGORYLINKS on a plain folder: links only is not a "
             "known count; no count, no version", s)
    url = "%s/category/%s/links" % (CAP, PL)
    c.step("FETCHCATEGORYLINKS", result(cat(PL, ROOT, "Plain", FT_NONE, version=9,
                                            embedded=emb(links=[link(L(0x331), PL, I(0x301), "Shirt")])),
                                        http_ok(url)), url=url, body=depth_body(0))

    # A known folder refetched with a newer version.
    s = Skel()
    FK = s.cat(C(0x340), ROOT, "Known", FT_NONE, 5)
    s.item(item(I(0x341), FK, "One"))
    s.item(item(I(0x342), FK, "Two"))
    c = Case("s01f_known_folder_newer", 1, "FETCHCATEGORYCHILDREN depth 0 of a known folder (v5) answered "
             "with v6 and three children: count and version set, existing items re-created", s)
    url = url_children(FK, 0)
    c.step("FETCHCATEGORYCHILDREN", result(cat(FK, ROOT, "Known", version=6, embedded=emb(
        categories={}, links={}, items=[item(I(0x341), FK, "One"), item(I(0x342), FK, "Two"),
                                        item(I(0x343), FK, "Three")])), http_ok(url)), target=FK, url=url,
        body=depth_body(0))

    # No owner in the category map: the new row gets a null owner.
    s = Skel()
    c = Case("s01g_category_without_agent_id", 1, "A new category without agent_id gets a null owner; one "
             "with agent_id gets that owner", s)
    NEW = C(0x350)
    url = url_children(NEW, 0)
    c.step("FETCHCATEGORYCHILDREN", result(cat(NEW, ROOT, "Ownerless", version=2, agent=False, embedded=emb(
        categories=[cat(C(0x351), NEW, "Owned child", extra={"agent_id": OTHER}, agent=False)], links={}, items={})),
        http_ok(url)), target=NEW, url=url, body=depth_body(0))


# ---- set 2: recursive depth 50 with a 52-level chain -------------------------


def set2():
    s = Skel()
    top = C(0x1000)
    s.cat(top, ROOT, "Deep 0")
    node = None
    for lvl in range(52, -1, -1):
        cid = C(0x1000 + lvl)
        parent = ROOT if lvl == 0 else C(0x1000 + lvl - 1)
        sub = {} if node is None else {node["category_id"]: node}
        node = cat(cid, parent, "Deep %d" % lvl, version=100 + lvl, embedded=emb(
            categories=sub, links={}, items=[item(I(0x1000 + lvl), cid, "Leaf %d" % lvl)]))
    c = Case("s02_recursive_depth50_chain52", 2, "FETCHCATEGORYCHILDREN depth 50 of a 53-level chain: "
             "versions set at depth >= 0 (levels 0-50), not at levels 51 and 52; counts known everywhere", s)
    url = url_children(top, 50)
    c.step("FETCHCATEGORYCHILDREN", result(node, http_ok(url)), target=top, url=url, body=depth_body(50),
           compact=True)


# ---- set 3: subset fetch ------------------------------------------------------


def set3():
    s = Skel()
    P = s.cat(C(0x400), ROOT, "Parent", FT_NONE, 4)
    C1 = s.cat(C(0x401), P, "Child one")
    C2 = s.cat(C(0x402), P, "Child two")
    G1 = C(0x403)
    body = cat(P, ROOT, "Parent renamed", version=9, embedded=emb(
        categories=[cat(C1, P, "Child one", version=5, embedded=emb(categories={}, links={}, items=[
            item(I(0x401), C1, "In child one")])),
            cat(C2, P, "Child two", version=6, embedded=emb(categories=[
                cat(G1, C2, "Grandchild", version=2, embedded=emb(categories={}, links={}, items={}))],
                links={}, items={}))],
        items=[item(I(0x400), P, "Directly in parent")], links={}))
    c = Case("s03a_subset_depth50", 3, "FETCHCATEGORYSUBSET: the parent itself is never applied; its "
             "_embedded children are parsed at depth 49 (versions set)", s)
    url = url_children(P, 50) + "&children=%s,%s" % (C1, C2)
    c.step("FETCHCATEGORYSUBSET", result(body, http_ok(url)), target=P, url=url, body=depth_body(50))

    c = Case("s03b_subset_without_embedded", 3, "FETCHCATEGORYSUBSET answered without _embedded: nothing", s)
    c.step("FETCHCATEGORYSUBSET", result(cat(P, ROOT, "Parent", version=9), http_ok(url)), target=P, url=url,
           body=depth_body(50))

    c = Case("s03c_subset_depth0", 3, "FETCHCATEGORYSUBSET at request depth 0: children at depth -1 get rows "
             "but no version", s)
    url0 = url_children(P, 0) + "&children=%s" % C1
    c.step("FETCHCATEGORYSUBSET", result(cat(P, ROOT, "Parent", version=9, embedded=emb(categories=[
        cat(C1, P, "Child one", version=5, embedded=emb(categories={}, links={}, items={}))])), http_ok(url0)),
        target=P, url=url0, body=depth_body(0))


# ---- set 4: links and their _embedded targets ---------------------------------


def set4():
    s = Skel()
    OUT = s.cat(C(0x500), MYOUTFITS, "Outfit", FT_OUTFIT, -1, -1)
    X = C(0x510)
    jacket = item(I(0x501), CLOTHING, "Jacket", AT_CLOTHING, IT_WEARABLE, flags=5)
    l1 = link(L(0x501), OUT, I(0x501), "Jacket", embedded=emb(single_item=jacket))
    l2 = link(L(0x502), OUT, X, "Accessories folder", AT_LINK_FOLDER, IT_CATEGORY,
              embedded=emb(single_category=cat(X, CLOTHING, "Accessories", version=3)))
    l3 = link(L(0x503), OUT, I(0x5FF), "Broken")
    c = Case("s04a_outfit_links_embedded", 4, "Outfit links with an embedded item, an embedded folder (folder "
             "link) and a broken link (target unknown, nothing embedded)", s)
    url = url_children(OUT, 0)
    c.step("FETCHCATEGORYCHILDREN", result(cat(OUT, MYOUTFITS, "Outfit", FT_OUTFIT, version=4,
                                               embedded=emb(links=[l1, l2, l3])), http_ok(url)),
           target=OUT, url=url, body=depth_body(0))

    # Write: links created in COF; only those in _created_items apply, and the
    # embedded target item is not in that list.
    s = Skel()
    s.item(jacket)
    s.item(item(I(0x504), CLOTHING, "Pants", AT_CLOTHING, IT_WEARABLE, flags=6))
    s.item(link(L(0x5A0), COF, I(0x504), "Pants"))
    shoes = item(I(0x505), CLOTHING, "Shoes", AT_CLOTHING, IT_WEARABLE, flags=7)
    res = {
        "_created_items": [L(0x5A1), L(0x5A2)],
        "_updated_category_versions": {COF: 12},
        "_embedded": emb(links=[
            link(L(0x5A1), COF, I(0x501), "Jacket", embedded=emb(single_item=jacket)),
            link(L(0x5A2), COF, I(0x505), "Shoes", embedded=emb(single_item=shoes)),
            link(L(0x5A3), COF, I(0x504), "Not created")]),
    }
    c = Case("s04b_create_links_created_filter", 4, "CREATEINVENTORY links into COF: links in _created_items "
             "apply, another embedded link and the embedded targets do not; COF +2 accounted (count only), "
             "version from _updated_category_versions; one callback per created id", s)
    url = "%s/category/%s?tid=%s" % (CAP, COF, "00000000-0000-4000-8000-00000000f001")
    c.step("CREATEINVENTORY", result(res, http_ok(url)), target=COF, url=url, body={"links": [
        {"linked_id": I(0x501), "type": AT_LINK, "inv_type": IT_WEARABLE, "name": "Jacket", "desc": ""},
        {"linked_id": I(0x505), "type": AT_LINK, "inv_type": IT_WEARABLE, "name": "Shoes", "desc": ""}]})

    # Fetch of a link: top-level link map, callback reports the target.
    s = Skel()
    s.item(jacket)
    lk = link(L(0x506), COF, I(0x501), "Jacket", embedded=emb(single_item=item(I(0x501), CLOTHING, "Jacket v2",
                                                                              AT_CLOTHING, IT_WEARABLE, flags=5)))
    c = Case("s04c_fetch_link_toplevel", 4, "FETCHITEM of a link: parsed as a link with its embedded target; "
             "the callback gets linked_id", s)
    url = url_item(L(0x506))
    c.step("FETCHITEM", result(lk, http_ok(url)), target=L(0x506), url=url)

    # A link fetched while its target is in the skeleton as a link (link to link).
    s = Skel()
    s.item(link(L(0x507), CLOTHING, I(0x501), "Existing link"))
    c = Case("s04d_link_update_existing", 4, "UPDATEITEM answered with an existing link: update path, no "
             "permission reset", s)
    url = url_item(L(0x507))
    c.step("UPDATEITEM", result(dict(link(L(0x507), CLOTHING, I(0x501), "Renamed link"),
                                     _updated_category_versions={CLOTHING: 8}), http_ok(url)),
           target=L(0x507), url=url, body={"name": "Renamed link"})


# ---- set 5: stale and version rules ------------------------------------------


def set5():
    F = C(0x600)

    def skel(version, descendents=None):
        s = Skel()
        s.cat(F, ROOT, "Versioned", FT_NONE, version, descendents)
        s.item(item(I(0x601), F, "Kept"))
        return s

    def answer(version):
        m = cat(F, ROOT, "Versioned", version=version, embedded=emb(categories=[cat(C(0x610), F, "Would be new")],
                                                                 links={}, items=[item(I(0x601), F, "Kept"),
                                                                                  item(I(0x602), F, "New")]))
        return m

    url = url_children(F, 0)
    c = Case("s05a_stale_known_folder", 5, "FIRE-33337: known v10 with a known count answered with v8: the "
             "model version goes to -1, fetch() is called, nothing of the folder or its _embedded applies",
             skel(10))
    c.step("FETCHCATEGORYCHILDREN", result(answer(8), http_ok(url)), target=F, url=url, body=depth_body(0))

    c = Case("s05b_lower_version_count_unknown", 5, "Known v10 but count unknown, answered with v8: not "
             "stale; the version goes down to 8 (warning only)", skel(10, -1))
    c.step("FETCHCATEGORYCHILDREN", result(answer(8), http_ok(url)), target=F, url=url, body=depth_body(0))

    c = Case("s05c_higher_version", 5, "Known v10 answered with v12: applied", skel(10))
    c.step("FETCHCATEGORYCHILDREN", result(answer(12), http_ok(url)), target=F, url=url, body=depth_body(0))

    c = Case("s05d_version_minus_one", 5, "Answered with version -1: count set, version not", skel(10))
    c.step("FETCHCATEGORYCHILDREN", result(answer(-1), http_ok(url)), target=F, url=url, body=depth_body(0))

    c = Case("s05e_version_missing", 5, "Answered without a version key: count set, version not", skel(10))
    c.step("FETCHCATEGORYCHILDREN", result(answer(None), http_ok(url)), target=F, url=url, body=depth_body(0))

    c = Case("s05f_viewer_version_unknown", 5, "Viewer version -1 answered with v8: applied", skel(-1, -1))
    c.step("FETCHCATEGORYCHILDREN", result(answer(8), http_ok(url)), target=F, url=url, body=depth_body(0))

    c = Case("s05g_stale_twice_same_frame", 5, "Two stale answers in the same frame: the second fetch() is "
             "inside the 10 s request guard (would_start false)", skel(10))
    c.step("FETCHCATEGORYCHILDREN", result(answer(8), http_ok(url)), target=F, url=url, body=depth_body(0))
    s2 = answer(7)
    c.step("FETCHCATEGORYCHILDREN", result(s2, http_ok(url)), target=F, url=url, body=depth_body(0), same_frame=True)

    c = Case("s05h_stale_on_update_category", 5, "UPDATECATEGORY answered with a lower version than the "
             "model's: the same stale path on a write", skel(10))
    urlc = url_cat(F)
    c.step("UPDATECATEGORY", result(cat(F, ROOT, "Versioned renamed", version=9), http_ok(urlc)), target=F,
           url=urlc, body={"name": "Versioned renamed"})

    c = Case("s05i_depth_minus_one_known_count", 5, "Fetch at request depth -1 (one level below the cut): "
             "count set, version not", skel(-1, -1))
    c.step("FETCHCATEGORYCHILDREN", result(answer(8), http_ok(url)), target=F, url=url, body=depth_body(-1))


# ---- set 6: items -------------------------------------------------------------


def set6():
    s = Skel()
    s.item(item(I(0x701), OBJECTS, "Box", AT_OBJECT, IT_OBJECT))
    url = url_item(I(0x701))
    c = Case("s06a_fetch_item_existing", 6, "FETCHITEM of a known item: applied as created (fetch), "
             "complete; callback item_id", s)
    c.step("FETCHITEM", result(item(I(0x701), OBJECTS, "Box renamed", AT_OBJECT, IT_OBJECT,
                                    extra={"running": True}), http_ok(url)), target=I(0x701), url=url)

    c = Case("s06b_fetch_item_new", 6, "FETCHITEM of an unknown item", Skel())
    url = url_item(I(0x702))
    c.step("FETCHITEM", result(item(I(0x702), TEXTURES, "Fresh texture"), http_ok(url)), target=I(0x702), url=url)

    c = Case("s06c_lost_item_null_parent", 6, "FETCHITEM answered with a null parent: lost item, parent set "
             "to Lost And Found and MoveInventoryItem sent before it is created", Skel())
    url = url_item(I(0x703))
    c.step("FETCHITEM", result(item(I(0x703), NULL, "Parentless", AT_NOTECARD, IT_NOTECARD), http_ok(url)),
           target=I(0x703), url=url)

    s = Skel()
    s.drop(LAF)
    c = Case("s06d_lost_item_no_lost_and_found", 6, "Lost item without a Lost And Found folder: no server "
             "move; created with a null parent", s)
    c.step("FETCHITEM", result(item(I(0x703), NULL, "Parentless", AT_NOTECARD, IT_NOTECARD), http_ok(url)),
           target=I(0x703), url=url)

    c = Case("s06e_item_unknown_parent", 6, "FETCHITEM of an item whose parent folder the model does not "
             "know: passed to updateItem as is (parent_known false)", Skel())
    url = url_item(I(0x704))
    c.step("FETCHITEM", result(item(I(0x704), C(0x7FF), "Orphaned", AT_LANDMARK, IT_LANDMARK), http_ok(url)),
           target=I(0x704), url=url)

    s = Skel()
    M = s.cat(C(0x710), ROOT, "Meshes")
    c = Case("s06f_mesh_gltf_items", 6, "Mesh, GLTF and GLTF binary items reach updateItem (the model skips "
             "them); a settings item next to them", s)
    url = url_children(M, 0)
    c.step("FETCHCATEGORYCHILDREN", result(cat(M, ROOT, "Meshes", version=3, embedded=emb(categories={}, links={}, items=[
        item(I(0x711), M, "Mesh", AT_MESH, IT_MESH), item(I(0x712), M, "Gltf", AT_GLTF, IT_GLTF),
        item(I(0x713), M, "Gltf bin", AT_GLTF_BIN, IT_GLTF_BIN),
        item(I(0x714), M, "Sky", AT_SETTINGS, IT_SETTINGS, flags=1)])), http_ok(url)),
        target=M, url=url, body=depth_body(0))

    c = Case("s06g_lost_link_null_parent", 6, "A fetched link with a null parent: lost, permissions forced, "
             "server move", Skel())
    url = url_item(L(0x705))
    c.step("FETCHITEM", result(link(L(0x705), NULL, I(0x701), "Lost link", itype=IT_OBJECT), http_ok(url)),
           target=L(0x705), url=url)

    s = Skel()
    s.item(item(I(0x706), OBJECTS, "Script", AT_LSL, IT_LSL, perm=perms(owner_mask=0x82000, next_mask=0)))
    c = Case("s06h_item_fields", 6, "FETCHITEM with every field the parser reads: shadow_id, thumbnail, "
             "favorite, sale_info with perm_mask, script runtime, running/faulted, a name with a pipe", s)
    url = url_item(I(0x706))
    fields = item(I(0x706), OBJECTS, "Script | one", AT_LSL, IT_LSL, flags=0x1, desc="A desc",
                  sale=(2, 10), perm=perms(owner_mask=0x82000, next_mask=0, group_mask=0x2000, everyone=0x8000),
                  extra={"thumbnail": {"asset_id": E(0x7AA)}, "favorite": {"toggled": True},
                         "script": {"runtime": "lua"}, "running": True, "faulted": True})
    fields["sale_info"]["perm_mask"] = 0x2000
    del fields["asset_id"]
    fields["shadow_id"] = U("e0000000-0000-4000-8000-000000000706")
    c.step("FETCHITEM", result(fields, http_ok(url)), target=I(0x706), url=url)


# ---- set 7: orphans -----------------------------------------------------------


def set7():
    s = Skel()
    s.item(item(I(0x801), OBJECTS, "Known"))
    res = {"_embedded": emb(
        items=[item(I(0x802), NULL, "Orphan with null parent"), item(I(0x803), C(0x8FF), "Orphan, parent unknown"),
               item(I(0x801), LAF, "Known, moved to Lost And Found")],
        categories=[cat(C(0x810), C(0x8FE), "Orphan folder", version=2,
                        embedded=emb(categories={}, links={}, items=[item(I(0x811), C(0x810), "In orphan folder")]))],
        links=[link(L(0x801), NULL, I(0x801), "Orphan link")])}
    c = Case("s07_orphans", 7, "FETCHORPHANS: no depth in the request (depth 50), a top level without "
             "category_id: _embedded parsed at depth 50; null parents go to Lost And Found with a server move", s)
    url = CAP + "/orphans"
    c.step("FETCHORPHANS", result(res, http_ok(url)), url=url)


# ---- set 8: write responses ---------------------------------------------------


def set8():
    # Create category: the reply is the parent with the new child embedded.
    s = Skel()
    P = s.cat(C(0x900), ROOT, "Projects", FT_NONE, 5)
    s.item(item(I(0x901), P, "Plan"))
    N = C(0x910)
    res = cat(P, ROOT, "Projects", version=6, embedded=emb(categories=[cat(N, P, "New folder", version=1)]),
              extra={"_created_categories": [N], "_updated_category_versions": {P: 6}})
    c = Case("s08a_create_category", 8, "CREATEINVENTORY of a folder: parent updated (copied back version and "
             "count), new folder created, parent +1 accounted count-only, version 6 from AIS; callback per "
             "created category", s)
    url = "%s/category/%s?tid=%s" % (CAP, P, "00000000-0000-4000-8000-00000000f002")
    c.step("CREATEINVENTORY", result(res, http_ok(url)), target=P, url=url,
           body={"categories": [{"name": "New folder", "type_default": -1, "parent_id": P}]})

    # Create with both categories and items: the callback order.
    s = Skel()
    res = {"_created_categories": [C(0x921), C(0x920)], "_created_items": [I(0x921), I(0x920)],
           "_updated_category_versions": {OBJECTS: 4},
           "_embedded": emb(categories=[cat(C(0x920), OBJECTS, "Made A", version=1),
                                        cat(C(0x921), OBJECTS, "Made B", version=1)],
                            items=[item(I(0x920), OBJECTS, "Made item A"), item(I(0x921), OBJECTS, "Made item B")])}
    c = Case("s08b_create_categories_and_items", 8, "CREATEINVENTORY creating folders and items: callbacks for "
             "the categories first, then the items, in array order", s)
    url = "%s/category/%s?tid=%s" % (CAP, OBJECTS, "00000000-0000-4000-8000-00000000f003")
    c.step("CREATEINVENTORY", result(res, http_ok(url)), target=OBJECTS, url=url, body={"categories": []})

    # Slam COF.
    s = Skel()
    s.cat(COF, ROOT, "Current Outfit", FT_COF, 13)
    s.item(item(I(0x931), CLOTHING, "Skirt", AT_CLOTHING, IT_WEARABLE, flags=12))
    s.item(item(I(0x932), CLOTHING, "Gloves", AT_CLOTHING, IT_WEARABLE, flags=9))
    s.item(link(L(0x93A), COF, I(0x931), "Skirt"))
    s.item(link(L(0x93B), COF, I(0x932), "Gloves"))
    res = {"_created_items": [L(0x93C), L(0x93D)], "_category_items_removed": [L(0x93A), L(0x93B)],
           "_removed_items": [L(0x93B)], "_updated_category_versions": {COF: 15},
           "_embedded": emb(links=[link(L(0x93C), COF, I(0x931), "Skirt"), link(L(0x93D), COF, I(0x932), "Gloves")])}
    c = Case("s08c_slam_cof", 8, "SLAMFOLDER on COF: two links replaced by two new ones; the removed lists "
             "merge (duplicates collapse); COF delta 0 accounted, version 13 -> 15", s)
    url = "%s/category/%s/links?tid=%s" % (CAP, COF, "00000000-0000-4000-8000-00000000f004")
    c.step("SLAMFOLDER", result(res, http_ok(url)), target=COF, url=url,
           body=[{"linked_id": I(0x931), "type": AT_LINK}, {"linked_id": I(0x932), "type": AT_LINK}])

    # Update item.
    s = Skel()
    s.item(item(I(0x941), TEXTURES, "Old name"))
    c = Case("s08d_update_item", 8, "UPDATEITEM: top-level item updated, parent delta 0 accounted, version "
             "from AIS; callback null", s)
    url = url_item(I(0x941))
    c.step("UPDATEITEM", result(dict(item(I(0x941), TEXTURES, "New name"), _updated_category_versions={TEXTURES: 6}),
                                http_ok(url)), target=I(0x941), url=url, body={"name": "New name"})

    # Update category, with and without _updated_categories.
    s = Skel()
    F = s.cat(C(0x950), ROOT, "Rename me", FT_NONE, 3)
    url = url_cat(F)
    c = Case("s08e_update_category_with_list", 8, "UPDATECATEGORY with _updated_categories: one callback per "
             "listed id (FIRE-34169)", s)
    c.step("UPDATECATEGORY", result(cat(F, ROOT, "Renamed", version=4, extra={
        "_updated_categories": [F, C(0x951)], "_updated_category_versions": {F: 4}}), http_ok(url)),
        target=F, url=url, body={"name": "Renamed"})
    c = Case("s08f_update_category_no_list", 8, "UPDATECATEGORY without _updated_categories: callback "
             "category_id", s)
    c.step("UPDATECATEGORY", result(cat(F, ROOT, "Renamed", version=4, extra={
        "_updated_category_versions": {F: 4}}), http_ok(url)), target=F, url=url, body={"name": "Renamed"})

    # Remove item and remove category.
    s = Skel()
    s.item(item(I(0x961), TEXTURES, "Doomed"))
    c = Case("s08g_remove_item", 8, "REMOVEITEM: deletion, parent -1 accounted count-only, version from AIS",
             s)
    url = url_item(I(0x961))
    c.step("REMOVEITEM", result({"_category_items_removed": [I(0x961)], "_updated_category_versions": {TEXTURES: 6}},
                                http_ok(url)), target=I(0x961), url=url)

    s = Skel()
    D = s.cat(C(0x970), OBJECTS, "Doomed folder", FT_NONE, 2)
    s.item(item(I(0x971), D, "Inside"))
    c = Case("s08h_remove_category", 8, "REMOVECATEGORY: the folder and its item removed; parent -1 "
             "accounted; the folder's own delta is skipped (not in the version list)", s)
    url = url_cat(D)
    c.step("REMOVECATEGORY", result({"_categories_removed": [D], "_category_items_removed": [I(0x971)],
                                     "_updated_category_versions": {OBJECTS: 4}}, http_ok(url)), target=D, url=url)

    # Purge Trash: every removed list, unknown ids, duplicates.
    s = Skel()
    T1 = s.cat(C(0x981), TRASH, "Trashed folder", FT_NONE, 1)
    T2 = s.cat(C(0x982), TRASH, "Trashed folder 2", FT_NONE, 1)
    s.item(item(I(0x981), TRASH, "Trashed item"))
    s.item(item(I(0x982), TRASH, "Trashed item 2"))
    s.item(item(I(0x983), T1, "Deep trashed item"))
    s.item(link(L(0x981), TRASH, I(0x9FF), "Broken trashed link"))
    res = {"_categories_removed": [T1, T2, C(0x9FE)],
           "_category_items_removed": [I(0x981), I(0x982), I(0x983), I(0x9FD), I(0x982)],
           "_removed_items": [I(0x982)], "_broken_links_removed": [L(0x981), L(0x9FC)],
           "_updated_category_versions": {TRASH: 14, T1: 2}}
    c = Case("s08i_purge_trash", 8, "PURGEDESCENDENTS: categories, items, broken links; unknown ids are "
             "dropped, duplicates collapse; Trash -5 accounted", s)
    url = "%s/category/%s/children" % (CAP, TRASH)
    c.step("PURGEDESCENDENTS", result(res, http_ok(url)), target=TRASH, url=url)

    # _updated_category_versions: higher, lower, equal, -1, unknown id, viewer unknown.
    s = Skel()
    A = s.cat(C(0x9A1), ROOT, "Higher", FT_NONE, 5)
    B = s.cat(C(0x9A2), ROOT, "Lower", FT_NONE, 9)
    Cc = s.cat(C(0x9A3), ROOT, "Equal", FT_NONE, 4)
    D = s.cat(C(0x9A4), ROOT, "Minus one", FT_NONE, 3)
    F = s.cat(C(0x9A6), ROOT, "Viewer unknown", FT_NONE, -1, -1)
    s.item(item(I(0x9A1), A, "In higher"))
    s.item(item(I(0x9A2), B, "In lower"))
    res = {"_category_items_removed": [I(0x9A1), I(0x9A2)],
           "_updated_category_versions": {A: 7, B: 6, Cc: 4, D: -1, C(0x9A5): 3, F: 5}}
    c = Case("s08j_category_versions_matrix", 8, "_updated_category_versions: higher (applied), lower "
             "(ignored), equal, -1 (fetch(360)), unknown id (skipped), viewer unknown (applied); deltas "
             "accounted count-only for A and B", s)
    url = url_item(I(0x9A1))
    c.step("REMOVEITEM", result(res, http_ok(url)), target=I(0x9A1), url=url)

    # Out-of-order responses for COF in one frame.
    s = Skel()
    s.cat(COF, ROOT, "Current Outfit", FT_COF, 10)
    s.item(item(I(0x9B1), CLOTHING, "Socks", AT_CLOTHING, IT_WEARABLE, flags=7))
    s.item(item(I(0x9B2), CLOTHING, "Scarf", AT_CLOTHING, IT_WEARABLE, flags=5))
    url = "%s/category/%s?tid=%s" % (CAP, COF, "00000000-0000-4000-8000-00000000f005")
    newer = {"_created_items": [L(0x9B2)], "_updated_category_versions": {COF: 12},
             "_embedded": emb(links=[link(L(0x9B2), COF, I(0x9B2), "Scarf")])}
    older = {"_created_items": [L(0x9B1)], "_updated_category_versions": {COF: 11},
             "_embedded": emb(links=[link(L(0x9B1), COF, I(0x9B1), "Socks")])}
    c = Case("s08k_cof_out_of_order", 8, "Two COF link creations answered out of order in one frame: v12 "
             "then v11; the older version is ignored (FIRE-33455), counts accounted for both", s)
    c.step("CREATEINVENTORY", result(newer, http_ok(url)), target=COF, url=url, body={"links": []})
    c.step("CREATEINVENTORY", result(older, http_ok(url)), target=COF, url=url, body={"links": []},
           same_frame=True)

    # Accounting refused: the viewer's count disagrees with the server's.
    s = Skel()
    M = s.cat(C(0x9C0), ROOT, "Mismatched", FT_NONE, 5, 7)
    s.item(item(I(0x9C1), M, "Only child"))
    c = Case("s08l_accounting_count_mismatch", 8, "accountForUpdate refused (server count 7, viewer 1): "
             "count unchanged; version still from AIS", s)
    url = url_item(I(0x9C1))
    c.step("REMOVEITEM", result({"_category_items_removed": [I(0x9C1)], "_updated_category_versions": {M: 6}},
                                http_ok(url)), target=I(0x9C1), url=url)

    # Copy a library folder.
    s = Skel()
    DEST = s.cat(C(0x9D0), ROOT, "Destination", FT_NONE, 2)
    NEWF = C(0x9D1)
    res = cat(NEWF, DEST, "Library outfit", FT_OUTFIT, version=1, embedded=emb(
        categories={}, links={}, items=[item(I(0x9D1), NEWF, "Copied shirt", AT_CLOTHING, IT_WEARABLE, flags=4),
                                        item(I(0x9D2), NEWF, "Not created")]),
        extra={"_created_categories": [NEWF], "_created_items": [I(0x9D1)],
               "_updated_category_versions": {DEST: 3}})
    c = Case("s08m_copy_library_category", 8, "COPYLIBRARYCATEGORY: new top-level folder created with the "
             "embedded items AIS lists as created; callback category_id", s)
    url = "%s/category/%s?tid=%s,depth=0" % (LIBCAP, C(0x9DF), "00000000-0000-4000-8000-00000000f006")
    c.step("COPYLIBRARYCATEGORY", result(res, http_ok(url)), target=DEST, url=url)

    # Created ids that are not embedded, and updated embedded objects that are ignored.
    s = Skel()
    s.item(item(I(0x9E1), TEXTURES, "Sibling"))
    res = dict(item(I(0x9E2), TEXTURES, "Updated top level"), _created_items=[I(0x9E9)],
               _updated_category_versions={TEXTURES: 6},
               _embedded=emb(items=[item(I(0x9E1), TEXTURES, "Sibling changed but not created")]))
    s.item(item(I(0x9E2), TEXTURES, "Top level"))
    c = Case("s08n_update_item_embedded_filter", 8, "UPDATEITEM with an embedded item not in _created_items "
             "(ignored) and a created id with no object", s)
    url = url_item(I(0x9E2))
    c.step("UPDATEITEM", result(res, http_ok(url)), target=I(0x9E2), url=url, body={"name": "Updated top level"})

    # The callback contract for every command type.
    every = {"category_id": C(0x9F1), "item_id": I(0x9F1), "linked_id": I(0x9F2),
             "_created_categories": [C(0x9F3), C(0x9F2)], "_created_items": [I(0x9F4), I(0x9F3)],
             "_updated_categories": [C(0x9F5), C(0x9F4)]}
    types = ["COPYINVENTORY", "SLAMFOLDER", "REMOVECATEGORY", "REMOVEITEM", "PURGEDESCENDENTS", "UPDATECATEGORY",
             "UPDATEITEM", "COPYLIBRARYCATEGORY", "CREATEINVENTORY", "FETCHITEM", "FETCHCATEGORYCHILDREN",
             "FETCHCATEGORYCATEGORIES", "FETCHCATEGORYSUBSET", "FETCHCOF", "FETCHORPHANS", "FETCHCATEGORYLINKS"]
    variants = [("all_ids", every, http_ok("https://cb.example/")),
                ("no_ids", {}, http_ok("https://cb.example/")),
                ("only_category_id", {"category_id": C(0x9F1)}, http_ok("https://cb.example/")),
                ("only_item_id", {"item_id": I(0x9F1)}, http_ok("https://cb.example/")),
                ("empty_created_lists", {"_created_categories": [], "_created_items": [], "_updated_categories": []},
                 http_ok("https://cb.example/")),
                ("failure_with_ids", every, http_fail("https://cb.example/", 400, "<llsd><map /></llsd>"))]
    for vname, body, http in variants:
        c = Case("s08z_callbacks_%s" % vname, 8, "The completion callback for every command type: %s (no "
                 "parent_id, so nothing is parsed)" % vname, Skel())
        for t in types:
            c.step(t, result(body, http), url="https://cb.example/", body=depth_body(0) if t.startswith("FETCH") else Undef())
    c = Case("s08z_callbacks_none_registered", 8, "A command without a callback: nothing is called", Skel())
    c.step("CREATEINVENTORY", result(every, http_ok("https://cb.example/")), url="https://cb.example/", callback=False)


# ---- set 9: error bodies ------------------------------------------------------


def set9():
    s = Skel()
    s.item(item(I(0xA01), TEXTURES, "Known"))
    url = url_item(I(0xA02))
    c = Case("s09a_fetch_item_404_with_item_id", 9, "FETCHITEM 404 with a map body carrying item_id: the "
             "error body becomes the result, nothing parsed (no parent_id), the callback reports item_id", s)
    body = {"error_code": 404, "error_description": "item not found", "item_id": I(0xA02)}
    c.step("FETCHITEM", result(body, http_fail(url, 404, llsd_xml(body, compact=True), "Not Found")),
           target=I(0xA02), url=url)

    url = url_item(I(0xA01))
    c = Case("s09b_remove_item_410", 9, "REMOVEITEM 410 Gone on a known item: refetch parent (no-op, version "
             "known), then onObjectDeletedFromServer with its defaults (parent version bumped)", s)
    c.step("REMOVEITEM", result({}, http_fail(url, 410, "", "Gone")), target=I(0xA01), url=url)

    c = Case("s09c_remove_item_410_unknown", 9, "REMOVEITEM 410 on an item the model does not know: nothing",
             Skel())
    c.step("REMOVEITEM", result({}, http_fail(url, 410, "", "Gone")), target=I(0xA01), url=url)

    s = Skel()
    G = s.cat(C(0xA10), C(0xA11), "Gone folder", FT_NONE, 3)
    s.cat(C(0xA11), ROOT, "Parent unknown version", FT_NONE, -1, -1)
    url = url_cat(G)
    c = Case("s09d_remove_category_410", 9, "REMOVECATEGORY 410 Gone: the folder is kept; its parent is "
             "refetched (fetch() starts: parent version unknown)", s)
    c.step("REMOVECATEGORY", result({}, http_fail(url, 410, "", "Gone")), target=G, url=url)

    s = Skel()
    BIG = s.cat(C(0xA20), ROOT, "Too big")
    c = Case("s09e_fetch_403_depth0_twice", 9, "FETCHCATEGORYCHILDREN 403 at depth 0 twice, then at depth 50: "
             "the alert notification only the first time in the process (function-static flag), then the "
             "plain one; nothing at depth 50", s)
    body403 = {"error_code": 403, "error_description": "too many descendents"}
    for depth in (0, 0, 50):
        u = url_children(BIG, depth)
        c.step("FETCHCATEGORYCHILDREN", result(body403, http_fail(u, 403, llsd_xml(body403, compact=True),
                                                                   "Forbidden")), target=BIG, url=u,
               body=depth_body(depth))

    url = url_children(BIG, 0)
    c = Case("s09f_success_non_map_body", 9, "200 with a non-map body: the adapter wraps it under content; "
             "nothing parsed; callback null", Skel())
    c.step("FETCHCATEGORYCHILDREN", result({"content": [1, "two", I(0xA30)]}, http_ok(url)), target=BIG, url=url,
           body=depth_body(0))

    c = Case("s09g_success_empty_body", 9, "200 with an empty body: {content: undef}", Skel())
    c.step("FETCHCATEGORYCHILDREN", result({"content": Undef()}, http_ok(url)), target=BIG, url=url,
           body=depth_body(0))

    c = Case("s09h_499_deserialize_failure", 9, "A body that is not LLSD XML: status 499, empty map", Skel())
    c.step("FETCHCATEGORYCHILDREN", result({}, http_fail(url, 499, "", "Failed to deserialize LLSD.")),
           target=BIG, url=url, body=depth_body(0))

    c = Case("s09i_500_with_body", 9, "500 with a map body: 5xx bodies are not parsed, the result is {}",
             Skel())
    c.step("FETCHCATEGORYCHILDREN", result({}, http_fail(url, 500, llsd_xml(cat(BIG, ROOT, "x", version=3),
                                                                            compact=True), "Internal Server Error")),
           target=BIG, url=url, body=depth_body(0))

    c = Case("s09j_4xx_non_map_body", 9, "400 with an array body: under content", Skel())
    c.step("CREATEINVENTORY", result({"content": ["bad request"]}, http_fail(url, 400, "", "Bad Request")),
           target=BIG, url=url, body={"categories": []})

    s = Skel()
    s.item(item(I(0xA01), TEXTURES, "Known"))
    c = Case("s09k_disconnected", 9, "gDisconnected: the coroutine calls back null at once, nothing parsed", s)
    c.step("FETCHITEM", result(item(I(0xA01), TEXTURES, "Would update"), http_ok(url_item(I(0xA01)))),
           target=I(0xA01), url=url_item(I(0xA01)), disconnected=True)

    c = Case("s09l_create_failure", 9, "CREATEINVENTORY 400 with a map body: one null callback", Skel())
    body = {"error_code": 400, "error_description": "bad folder"}
    c.step("CREATEINVENTORY", result(body, http_fail(url, 400, llsd_xml(body, compact=True), "Bad Request")),
           target=ROOT, url=url, body={"categories": []})

    s = Skel()
    W = s.cat(C(0xA40), ROOT, "Wrong", FT_NONE, 2)
    body = {"error_code": 404, "error_description": "category not found", "category_id": W, "parent_id": ROOT,
            "name": "Error body name", "version": 8}
    c = Case("s09m_error_body_with_category", 9, "A 404 map body with category_id and parent_id is parsed as "
             "a category (AISUpdate runs on error results): mock shape, from the parser", s)
    u = url_children(W, 0)
    c.step("FETCHCATEGORYCHILDREN", result(body, http_fail(u, 404, llsd_xml(body, compact=True), "Not Found")),
           target=W, url=u, body=depth_body(0))

    c = Case("s09n_transport_timeout", 9, "A transport failure (curl timeout): type 0 status 28, {}", Skel())
    c.step("FETCHCATEGORYCHILDREN", result({}, http_fail(url, 0, "", "Timeout was reached", status=28)),
           target=BIG, url=url, body=depth_body(0))

    s = Skel()
    s.item(item(I(0xA01), TEXTURES, "Known"))
    u = url_item(I(0xA01))
    c = Case("s09o_update_item_404_with_item", 9, "UPDATEITEM 404 whose body is a full item map: parsed as an "
             "update (error results are parsed)", s)
    body = dict(item(I(0xA01), TEXTURES, "From error body"), error_code=404)
    c.step("UPDATEITEM", result(body, http_fail(u, 404, llsd_xml(body, compact=True), "Not Found")),
           target=I(0xA01), url=u, body={"name": "x"})


# ---- set 10: large and the batch timer ----------------------------------------


class XorShift64Star:
    """xorshift64* (Vigna), the generator rung two's sweeps used."""

    def __init__(self, seed):
        self.s = seed & 0xFFFFFFFFFFFFFFFF or 1

    def next(self):
        x = self.s
        x ^= x >> 12
        x ^= (x << 25) & 0xFFFFFFFFFFFFFFFF
        x ^= x >> 27
        self.s = x
        return (x * 0x2545F4914F6CDD1D) & 0xFFFFFFFFFFFFFFFF

    def below(self, n):
        return (self.next() >> 32) % n


LARGE_SEED = 0x5EED0A15DA7E0010
ITEM_KINDS = [(AT_TEXTURE, IT_TEXTURE), (AT_LANDMARK, IT_LANDMARK), (AT_CLOTHING, IT_WEARABLE),
              (AT_OBJECT, IT_OBJECT), (AT_NOTECARD, IT_NOTECARD), (AT_LSL, IT_LSL), (AT_BODYPART, IT_WEARABLE),
              (AT_ANIMATION, IT_ANIMATION), (AT_GESTURE, IT_GESTURE), (AT_MESH, IT_MESH), (AT_SETTINGS, IT_SETTINGS)]


def set10():
    rng = XorShift64Star(LARGE_SEED)
    NF, NI, NL = 2000, 20000, 500
    base = 0x100000
    fid = [C(base + i) for i in range(NF)]
    parent = [ROOT] + [fid[(i - 1) // 10] for i in range(1, NF)]
    versions = [1 + rng.below(50) for _ in range(NF)]
    items_in = [[] for _ in range(NF)]
    for j in range(NI):
        f = j % NF
        at, it = ITEM_KINDS[rng.below(len(ITEM_KINDS))]
        flags = rng.below(16) if it == IT_WEARABLE else 0
        items_in[f].append(item(I(base + j), fid[f], "Item %05d" % j, at, it, flags=flags, created=1600000000 + j))
    links_in = [[] for _ in range(NF)]
    for k in range(NL):
        f = (k * 4) % NF
        links_in[f].append(link(L(base + k), fid[f], I(base + k), "Link %04d" % k, itype=IT_TEXTURE))
    children = [[] for _ in range(NF)]
    for i in range(1, NF):
        children[(i - 1) // 10].append(i)

    def build(i):
        return cat(fid[i], parent[i], "Folder %04d" % i, version=versions[i], embedded=emb(
            categories={fid[k]: build(k) for k in children[i]}, links=links_in[i], items=items_in[i]))

    s = Skel()
    s.cat(fid[0], ROOT, "Folder 0000")
    c = Case("s10a_large_recursive", 10, "FETCHCATEGORYCHILDREN depth 50 answered with 2,000 folders, 20,000 "
             "items and 500 links: op count, the 50-change observer flushes and the yields (digest and sample)",
             s, call_us=5, large=True, seed=LARGE_SEED, others_us=[0, 3000, 9500, 12000])
    url = url_children(fid[0], 50)
    c.step("FETCHCATEGORYCHILDREN", result(build(0), http_ok(url)), target=fid[0], url=url, body=depth_body(50),
           compact=True, gz=True)

    # Three mid-sized replies in one frame: the shared per-frame batch timer
    # (10 ms) runs across them while each has its own 8 ms task timer.
    s = Skel()
    tops = [C(0x2000 + 0x100 * n) for n in range(3)]
    for n, t in enumerate(tops):
        s.cat(t, ROOT, "Batch %d" % n)
    c = Case("s10b_batch_timer_same_frame", 10, "Three replies, the later ones in the frame the earlier one "
             "yielded to, while another AIS update resumes first in each new frame and runs 5 ms: the shared "
             "batch timer (armed by the other update) yields before the 8 ms task timer", s, call_us=100,
             others_us=[5000])
    for n, t in enumerate(tops):
        subs = [cat(C(0x2000 + 0x100 * n + 1 + k), t, "Sub %d.%d" % (n, k), version=1, embedded=emb(
            categories={}, links={}, items=[item(I(0x2000 + 0x100 * n + 0x10 * k + m), C(0x2000 + 0x100 * n + 1 + k),
                                                 "Item %d.%d.%d" % (n, k, m)) for m in range(4)]))
                for k in range(6)]
        u = url_children(t, 50)
        c.step("FETCHCATEGORYCHILDREN", result(cat(t, ROOT, "Batch %d" % n, version=2, embedded=emb(
            categories=subs, links={}, items={})), http_ok(u)), target=t, url=u, body=depth_body(50),
            same_frame=(n > 0))

    # The same three replies, each in its own frame.
    c2 = Case("s10c_batch_timer_separate_frames", 10, "The same three replies, one per frame", s, call_us=100)
    for st in c.steps:
        st2 = dict(st)
        st2["same_frame"] = False
        c2.steps.append(st2)


# ---- set 11: duplicate keys in XML maps ---------------------------------------


def set11():
    s = Skel()
    F = s.cat(C(0xB00), ROOT, "Duplicates")
    url = url_children(F, 0)

    def http():
        return http_ok(url)

    # duplicate "version": the last one wins
    body = Dup([("category_id", F), ("parent_id", ROOT), ("name", "Duplicates"), ("type_default", -1),
                ("agent_id", AGENT), ("version", 3), ("version", 9),
                ("_embedded", {"categories": {}, "links": {}, "items": {}})])
    c = Case("s11a_duplicate_version_key", 11, "A category map with version twice (3 then 9): the parser keeps "
             "the last", s)
    c.step("FETCHCATEGORYCHILDREN", result(body, http()), target=F, url=url, body=depth_body(0))

    # duplicate item id key inside "items": last entry wins, count = 1
    items = Dup([(I(0xB01), item(I(0xB01), F, "First")), (I(0xB01), item(I(0xB01), F, "Second"))])
    body = Dup([("category_id", F), ("parent_id", ROOT), ("name", "Duplicates"), ("type_default", -1),
                ("agent_id", AGENT), ("version", 4),
                ("_embedded", Dup([("categories", {}), ("links", {}), ("items", items)]))])
    c = Case("s11b_duplicate_item_key", 11, "The same item id twice in an items map: one entry (the last), "
             "count 1", s)
    c.step("FETCHCATEGORYCHILDREN", result(body, http()), target=F, url=url, body=depth_body(0))

    # duplicate "_embedded": the second replaces the first
    body = Dup([("category_id", F), ("parent_id", ROOT), ("name", "Duplicates"), ("type_default", -1),
                ("agent_id", AGENT), ("version", 5),
                ("_embedded", {"categories": {}, "links": {}, "items": {I(0xB02): item(I(0xB02), F, "Lost to dup")}}),
                ("_embedded", {"categories": {C(0xB03): cat(C(0xB03), F, "Kept", version=1)}})])
    c = Case("s11c_duplicate_embedded_key", 11, "_embedded twice: the second (categories only) replaces the "
             "first; count unknown, version not set", s)
    c.step("FETCHCATEGORYCHILDREN", result(body, http()), target=F, url=url, body=depth_body(0))

    # duplicate meta keys on a write
    s2 = Skel()
    s2.item(item(I(0xB11), TEXTURES, "Doomed one"))
    s2.item(item(I(0xB12), TEXTURES, "Doomed two"))
    u = url_item(I(0xB11))
    body = Dup([("_category_items_removed", [I(0xB11)]), ("_category_items_removed", [I(0xB12)]),
                ("_updated_category_versions", {TEXTURES: 9}), ("_updated_category_versions", {TEXTURES: 6}),
                ("_created_items", [I(0xB13)]), ("_created_items", [])])
    c = Case("s11d_duplicate_meta_keys", 11, "Meta keys twice on a write: only the last array/map of each "
             "counts", s2)
    c.step("REMOVEITEM", result(body, http_ok(u)), target=I(0xB11), url=u)

    # a duplicate key inside _updated_category_versions itself
    body = {"_category_items_removed": [I(0xB11)],
            "_updated_category_versions": Dup([(TEXTURES, 9), (TEXTURES, 6)])}
    c = Case("s11e_duplicate_version_entry", 11, "The same category twice in _updated_category_versions: the "
             "last (6) counts", s2)
    c.step("REMOVEITEM", result(body, http_ok(u)), target=I(0xB11), url=u)


# ---- set 12: seeded random combinations ---------------------------------------

RANDOM_SEED = 0x5EED0A15DA7E0012
RANDOM_CASES = 40
FETCH_TYPES = ["FETCHCATEGORYCHILDREN", "FETCHCATEGORYSUBSET", "FETCHCOF", "FETCHCATEGORYLINKS", "FETCHITEM",
               "FETCHORPHANS", "FETCHCATEGORYCATEGORIES"]
WRITE_TYPES = ["CREATEINVENTORY", "UPDATECATEGORY", "UPDATEITEM", "REMOVEITEM", "REMOVECATEGORY", "PURGEDESCENDENTS",
               "SLAMFOLDER", "COPYLIBRARYCATEGORY"]


def set12():
    """Random skeletons and replies from one xorshift64* stream (seed
    RANDOM_SEED, cases drawn in order): version relations (lower, equal,
    higher, -1, missing), _embedded keys present or not, request depths
    around the cut, created and removed lists naming known, embedded and
    unknown ids, null parents, failures, frames and concurrent updates."""
    rng = XorShift64Star(RANDOM_SEED)

    def pick(seq):
        return seq[rng.below(len(seq))]

    def chance(pct):
        return rng.below(100) < pct

    for k in range(RANDOM_CASES):
        base = 0x30000 + k * 0x100
        s = Skel()
        folders = [ROOT, CLOTHING, OBJECTS, COF, TRASH]
        known_items = []
        for n in range(1 + rng.below(6)):
            fid = C(base + n)
            parent = pick(folders)
            version = pick([-1, -1, 1 + rng.below(20)])
            desc = None if version < 0 or chance(85) else rng.below(5)
            ftype = FT_OUTFIT if chance(15) else FT_NONE
            s.cat(fid, parent, "R%d folder %d" % (k, n), ftype, version, -1 if version < 0 else desc)
            folders.append(fid)
        for n in range(rng.below(8)):
            iid = I(base + n)
            if chance(25) and known_items:
                s.item(link(L(base + n), pick(folders), pick(known_items), "R%d link %d" % (k, n)))
                known_items.append(L(base + n))
            else:
                at, it = pick(ITEM_KINDS)
                s.item(item(iid, pick(folders), "R%d item %d" % (k, n), at, it))
                known_items.append(iid)
        versions = {c["category_id"]: c["version"] for c in s.cats}
        fresh = [0x80]

        def new_id(maker):
            fresh[0] += 1
            return maker(base + fresh[0])

        def some_version(cid):
            v = versions.get(cid, -1)
            r = rng.below(6)
            if r == 0 or v < 0:
                return 1 + rng.below(25)
            return [v - 1, v, v + 1, v + 3, -1][r - 1]

        def rand_item(parent):
            if known_items and chance(40):
                iid = pick(known_items)
                if iid.startswith("b"):
                    return link(iid, parent, pick(known_items), "R%d relinked" % k)
            else:
                iid = new_id(I)
            at, it = pick(ITEM_KINDS)
            return item(iid, NULL if chance(5) else parent, "R%d it %s" % (k, iid[-3:]), at, it,
                        flags=rng.below(4))

        def rand_link(parent):
            target = pick(known_items) if known_items and chance(70) else new_id(I)
            emb_item = emb(single_item=item(target, CLOTHING, "Target %s" % target[-3:])) if chance(40) else None
            return link(new_id(L), parent, target, "R%d ln" % k, embedded=emb_item)

        def rand_cat(cid, parent, depth_left):
            m = cat(cid, parent, "R%d cat %s" % (k, cid[-3:]), FT_OUTFIT if chance(10) else FT_NONE,
                    version=None if chance(10) else some_version(cid))
            if depth_left > 0 and chance(85):
                e = {}
                if chance(80):
                    subs = [rand_cat(pick(folders[5:]) if len(folders) > 5 and chance(30) else new_id(C), cid,
                                     depth_left - 1) for _ in range(rng.below(3))]
                    e["categories"] = {x["category_id"]: x for x in subs}
                if chance(80):
                    ls = [rand_link(cid) for _ in range(rng.below(3))]
                    e["links"] = {x["item_id"]: x for x in ls}
                if chance(80):
                    its = [rand_item(cid) for _ in range(rng.below(4))]
                    e["items"] = {x["item_id"]: x for x in its}
                m["_embedded"] = e
            return m

        c = Case("s12_random_%02d" % k, 12, "Seeded random combination %d (set 12)" % k, s,
                 call_us=pick([20, 50, 100]), others_us=pick([[], [0], [4000], [0, 12000]]))
        for n in range(1 + rng.below(3)):
            if chance(55):
                ctype = pick(FETCH_TYPES)
                target = pick(folders)
                depth = pick([-1, 0, 0, 1, 2, 50])
                if ctype == "FETCHITEM":
                    body = Undef()
                    obj = rand_item(pick(folders)) if chance(70) else rand_link(pick(folders))
                    target = obj["item_id"]
                elif ctype == "FETCHORPHANS":
                    body = Undef()
                    obj = {"_embedded": rand_cat(target, ROOT, 2).get("_embedded", {})}
                else:
                    body = depth_body(0 if ctype in ("FETCHCOF", "FETCHCATEGORYLINKS") else depth)
                    if ctype == "FETCHCOF":
                        target = COF
                    parent = next((x["parent_id"] for x in s.cats if x["category_id"] == target), ROOT)
                    obj = rand_cat(target, parent, 3)
                    if ctype in ("FETCHCOF", "FETCHCATEGORYLINKS") and "_embedded" in obj:
                        obj["_embedded"] = {"links": obj["_embedded"].get("links", {})}
            else:
                ctype = pick(WRITE_TYPES)
                target = pick(folders)
                body = {"name": "x"}
                obj = {}
                if chance(50):
                    parent = next((x["parent_id"] for x in s.cats if x["category_id"] == target), ROOT)
                    obj = rand_cat(target, parent, 1)
                elif chance(40) and known_items:
                    target = pick(known_items)
                    obj = rand_item(pick(folders))
                    obj["item_id"] = target
                emb_ids_c = [x for x in obj.get("_embedded", {}).get("categories", {})]
                emb_ids_i = ([x for x in obj.get("_embedded", {}).get("items", {})]
                             + [x for x in obj.get("_embedded", {}).get("links", {})])
                if chance(60):
                    obj["_created_categories"] = [x for x in emb_ids_c if chance(70)] + ([new_id(C)] if chance(20) else [])
                if chance(60):
                    obj["_created_items"] = [x for x in emb_ids_i if chance(70)] + ([new_id(I)] if chance(20) else [])
                for key, pool in (("_categories_removed", folders[5:]), ("_category_items_removed", known_items),
                                  ("_removed_items", known_items), ("_broken_links_removed", known_items)):
                    if chance(25) and pool:
                        obj[key] = [pick(pool) for _ in range(1 + rng.below(2))] + ([new_id(I)] if chance(20) else [])
                if chance(70):
                    ucv = {}
                    for _ in range(1 + rng.below(3)):
                        cid = pick(folders) if chance(85) else new_id(C)
                        ucv[cid] = some_version(cid)
                    obj["_updated_category_versions"] = ucv
                if chance(20):
                    obj["_updated_categories"] = [target]
            if chance(10):
                code = pick([400, 403, 404, 410, 500])
                http = http_fail("https://random.example/", code, "", "error")
                if code >= 500:
                    obj = {}
            else:
                http = http_ok("https://random.example/")
            c.step(ctype, result(obj, http), target=target, url="https://random.example/", body=body,
                   same_frame=chance(30))


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    outdir = sys.argv[1]
    os.makedirs(outdir, exist_ok=True)
    for name in os.listdir(outdir):
        if name.endswith(".xml") or name.endswith(".xml.gz") or name == "index.txt":
            os.remove(os.path.join(outdir, name))
    for f in (set1, set2, set3, set4, set5, set6, set7, set8, set9, set10, set11, set12):
        f()
    ids = [c.id for c in CASES]
    if len(set(ids)) != len(ids):
        raise SystemExit("duplicate case ids")
    for c in CASES:
        c.write(outdir)
    with open(os.path.join(outdir, "index.txt"), "w", newline="\n") as f:
        f.write("# AIS update corpus (recorder R3a): case ids in run order; written by "
                "tools/inventory-fixtures/ais-update/gen_cases.py in vkstorm-core-rust\n")
        for c in CASES:
            f.write(c.id + "\n")
    print("%d cases written to %s" % (len(CASES), outdir))
    return 0


if __name__ == "__main__":
    sys.exit(main())
