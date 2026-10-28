// The iteration order of std::unordered_map<LLUUID, int> with the viewer's
// std::hash<LLUUID> (indra/llcommon/lluuid.h), for tests/unordered_order.rs.
// stdin: one operation per line, "i <32 hex>" insert (operator[]), "e <32 hex>"
// erase, "c" clear, "p" print the keys in iteration order on one line.
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <iostream>
#include <string>
#include <unordered_map>

struct Id { unsigned char b[16]; bool operator==(const Id& o) const { return std::memcmp(b, o.b, 16) == 0; } };
struct IdHash {
    size_t operator()(const Id& id) const noexcept {
        size_t h = 0;
        for (int i = 0; i < 16; i += 8) {
            size_t chunk = (size_t)id.b[i] | ((size_t)id.b[i+1] << 8) | ((size_t)id.b[i+2] << 16) | ((size_t)id.b[i+3] << 24) |
                           ((size_t)id.b[i+4] << 32) | ((size_t)id.b[i+5] << 40) | ((size_t)id.b[i+6] << 48) | ((size_t)id.b[i+7] << 56);
            h ^= (chunk * 0x9e3779b97f4a7c15ULL) ^ (h >> 31) ^ (h >> 47);
        }
        return h;
    }
};

static Id parse(const std::string& s) {
    Id id{};
    for (int i = 0; i < 16; ++i) id.b[i] = (unsigned char)std::stoul(s.substr(i * 2, 2), nullptr, 16);
    return id;
}

int main() {
    static_assert(sizeof(size_t) == 8, "64-bit size_t");
    std::unordered_map<Id, int, IdHash> m;
    std::string op, arg;
    while (std::cin >> op) {
        if (op == "i") { std::cin >> arg; m[parse(arg)] = 1; }
        else if (op == "e") { std::cin >> arg; m.erase(parse(arg)); }
        else if (op == "c") { m.clear(); }
        else if (op == "p") {
            bool first = true;
            for (const auto& kv : m) {
                if (!first) std::cout << ' ';
                first = false;
                for (int i = 0; i < 16; ++i) { char buf[3]; std::snprintf(buf, 3, "%02x", kv.first.b[i]); std::cout << buf; }
            }
            std::cout << '\n';
        }
    }
    return 0;
}
