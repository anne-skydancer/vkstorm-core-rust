// LLSD(string).asReal() exactly as indra/llcommon/llsd.cpp ImplString::asReal:
// istringstream >> double, then get() must report EOF, else 0.0.
// Input: one hex-encoded byte string per line on stdin.
// Output: the 16 hex digits of the resulting double's bits, one per line.
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <iostream>
#include <sstream>
#include <string>

static double as_real(const std::string& value)
{
    double v = 0.0;
    std::istringstream i_stream(value);
    i_stream >> v;
    int c = i_stream.get();
    return ((EOF == c) ? v : 0.0);
}

static int nybble(char c)
{
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    return c - 'A' + 10;
}

int main()
{
    std::string line;
    while (std::getline(std::cin, line))
    {
        if (!line.empty() && line.back() == '\r') line.pop_back();
        std::string bytes;
        for (size_t i = 0; i + 1 < line.size(); i += 2)
            bytes.push_back((char)(nybble(line[i]) * 16 + nybble(line[i + 1])));
        double v = as_real(bytes);
        std::uint64_t bits;
        std::memcpy(&bits, &v, sizeof bits);
        std::printf("%016llx\n", (unsigned long long)bits);
    }
    return 0;
}
