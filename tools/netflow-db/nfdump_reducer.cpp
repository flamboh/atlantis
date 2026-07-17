#include <arpa/inet.h>

#include <algorithm>
#include <array>
#include <bitset>
#include <cstdint>
#include <csignal>
#include <cstdlib>
#include <iostream>
#include <limits>
#include <stdexcept>
#include <string>
#include <string_view>
#include <unordered_set>
#include <vector>

namespace {

constexpr int kContractVersion = 1;
constexpr size_t kFieldCount = 15;
constexpr size_t kScopeCount = 10;
constexpr uint64_t kMaxInteger = std::numeric_limits<int64_t>::max();
constexpr uint64_t kMinIntegerMagnitude = kMaxInteger + 1;
constexpr std::string_view kInputContract = "nfdump-csv-15-v1";
constexpr std::string_view kOutputContract = "canonical-scopes-v1";
constexpr std::string_view kCsvHeader =
    "received,lastSeen,firstSeen,srcAddr,dstAddr,srcPort,dstPort,proto,packets,"
    "bytes,srcTos,dstTos,flows,minTTL,maxTTL";

enum Metric : size_t {
    FLOWS,
    FLOWS_TCP,
    FLOWS_UDP,
    FLOWS_ICMP,
    FLOWS_OTHER,
    PACKETS,
    PACKETS_TCP,
    PACKETS_UDP,
    PACKETS_ICMP,
    PACKETS_OTHER,
    BYTES,
    BYTES_TCP,
    BYTES_UDP,
    BYTES_ICMP,
    BYTES_OTHER,
    DURATION_SUM_MS,
    DURATION_COUNT,
    MIN_TTL_SUM,
    MIN_TTL_COUNT,
    MAX_TTL_SUM,
    MAX_TTL_COUNT,
    METRIC_COUNT,
};

struct Scope {
    int ipVersion = 0;
    const char* srcVisibility = nullptr;
    const char* dstVisibility = nullptr;
    std::array<uint64_t, METRIC_COUNT> metrics{};
    std::bitset<256> protocols;
    std::unordered_set<std::string> sourceAddresses;
    std::unordered_set<std::string> destinationAddresses;
    std::bitset<65536> sourcePorts;
    std::bitset<65536> destinationPorts;
};

uint64_t parseUnsigned(std::string_view value, std::string_view name, uint64_t maximum) {
    if (value.empty()) throw std::runtime_error(std::string(name) + " is empty");
    uint64_t result = 0;
    for (char character : value) {
        if (character < '0' || character > '9') {
            throw std::runtime_error(std::string(name) + " is not an unsigned integer");
        }
        const uint64_t digit = static_cast<uint64_t>(character - '0');
        if (result > (maximum - digit) / 10) {
            throw std::runtime_error(std::string(name) + " is out of range");
        }
        result = result * 10 + digit;
    }
    return result;
}

int64_t parseUnixMilliseconds(std::string_view value, std::string_view name) {
    if (value.empty()) throw std::runtime_error(std::string(name) + " is empty");
    bool negative = value.front() == '-';
    if (negative) value.remove_prefix(1);
    const size_t decimal = value.find('.');
    const std::string_view secondsText = value.substr(0, decimal);
    std::string_view fraction = decimal == std::string_view::npos
        ? std::string_view{}
        : value.substr(decimal + 1);
    if (secondsText.empty() || fraction.size() > 3) {
        throw std::runtime_error(std::string(name) + " must have millisecond precision");
    }
    const uint64_t maximumMagnitude = negative ? kMinIntegerMagnitude : kMaxInteger;
    uint64_t seconds = parseUnsigned(secondsText, name, maximumMagnitude / 1000);
    uint64_t milliseconds = 0;
    if (!fraction.empty()) {
        milliseconds = parseUnsigned(fraction, name, 999);
        for (size_t index = fraction.size(); index < 3; ++index) milliseconds *= 10;
    }
    uint64_t absolute = seconds * 1000 + milliseconds;
    if (absolute > maximumMagnitude) throw std::runtime_error(std::string(name) + " is out of range");
    if (!negative) return static_cast<int64_t>(absolute);
    if (absolute == kMinIntegerMagnitude) return std::numeric_limits<int64_t>::min();
    return -static_cast<int64_t>(absolute);
}

uint64_t durationMilliseconds(int64_t start, int64_t end) {
    if (end < start) throw std::runtime_error("time_end precedes time_start");
    uint64_t duration = 0;
    if (start < 0 && end >= 0) {
        duration = static_cast<uint64_t>(end) + static_cast<uint64_t>(-(start + 1)) + 1;
    } else {
        duration = static_cast<uint64_t>(end - start);
    }
    if (duration > kMaxInteger) throw std::runtime_error("duration exceeds signed 64-bit range");
    return duration;
}

void checkedAdd(uint64_t& target, uint64_t value, std::string_view name) {
    if (value > kMaxInteger || target > kMaxInteger - value) {
        throw std::runtime_error(std::string(name) + " exceeds signed 64-bit range");
    }
    target += value;
}

uint64_t checkedMultiply(uint64_t left, uint64_t right, std::string_view name) {
    if (left != 0 && right > kMaxInteger / left) {
        throw std::runtime_error(std::string(name) + " exceeds signed 64-bit range");
    }
    return left * right;
}

std::array<std::string_view, kFieldCount> splitCsvLine(const std::string& line) {
    std::array<std::string_view, kFieldCount> fields;
    size_t start = 0;
    for (size_t index = 0; index < kFieldCount; ++index) {
        const size_t comma = line.find(',', start);
        if (index + 1 == kFieldCount) {
            if (comma != std::string::npos) throw std::runtime_error("CSV row has too many fields");
            fields[index] = std::string_view(line).substr(start);
        } else {
            if (comma == std::string::npos) throw std::runtime_error("CSV row has too few fields");
            fields[index] = std::string_view(line).substr(start, comma - start);
            start = comma + 1;
        }
    }
    return fields;
}

int addressFamily(std::string_view address) {
    std::array<unsigned char, 16> bytes{};
    std::string text(address);
    if (inet_pton(AF_INET, text.c_str(), bytes.data()) == 1) return 4;
    if (inet_pton(AF_INET6, text.c_str(), bytes.data()) == 1) return 6;
    throw std::runtime_error("invalid IP address");
}

size_t exactScopeIndex(uint64_t tosBits) {
    static constexpr std::array<size_t, 4> indexes{4, 3, 2, 1};
    return indexes[tosBits & 3];
}

size_t protocolMetricOffset(uint64_t protocol) {
    if (protocol == 6) return 1;
    if (protocol == 17) return 2;
    if (protocol == 1 || protocol == 58) return 3;
    return 4;
}

bool allowsVisibility(uint64_t tos, const std::string& source, const std::string& destination) {
    const bool sourceAnonymized = (tos & 2) != 0;
    const bool destinationAnonymized = (tos & 1) != 0;
    if (!source.empty() && (source == "anonymized") != sourceAnonymized) return false;
    if (!destination.empty() && (destination == "anonymized") != destinationAnonymized) return false;
    return true;
}

uint16_t parsePort(std::string_view value) {
    if (value.find('.') != std::string_view::npos) return 0;
    return static_cast<uint16_t>(parseUnsigned(value, "port", 65535));
}

void addFlow(
    Scope& scope,
    uint64_t protocol,
    uint64_t packets,
    uint64_t bytes,
    uint64_t flowCount,
    uint64_t duration,
    uint64_t minTtl,
    uint64_t maxTtl,
    bool hasMinTtl,
    bool hasMaxTtl,
    const std::string& sourceAddress,
    const std::string& destinationAddress,
    uint16_t sourcePort,
    uint16_t destinationPort
) {
    const size_t offset = protocolMetricOffset(protocol);
    checkedAdd(scope.metrics[FLOWS], flowCount, "flows");
    checkedAdd(scope.metrics[FLOWS + offset], flowCount, "protocol flows");
    checkedAdd(scope.metrics[PACKETS], packets, "packets");
    checkedAdd(scope.metrics[PACKETS + offset], packets, "protocol packets");
    checkedAdd(scope.metrics[BYTES], bytes, "bytes");
    checkedAdd(scope.metrics[BYTES + offset], bytes, "protocol bytes");
    checkedAdd(scope.metrics[DURATION_SUM_MS], checkedMultiply(duration, flowCount, "duration sum"), "duration sum");
    checkedAdd(scope.metrics[DURATION_COUNT], flowCount, "duration count");
    if (hasMinTtl) {
        checkedAdd(scope.metrics[MIN_TTL_SUM], checkedMultiply(minTtl, flowCount, "min TTL sum"), "min TTL sum");
        checkedAdd(scope.metrics[MIN_TTL_COUNT], flowCount, "min TTL count");
    }
    if (hasMaxTtl) {
        checkedAdd(scope.metrics[MAX_TTL_SUM], checkedMultiply(maxTtl, flowCount, "max TTL sum"), "max TTL sum");
        checkedAdd(scope.metrics[MAX_TTL_COUNT], flowCount, "max TTL count");
    }
    scope.protocols.set(protocol);
    scope.sourceAddresses.insert(sourceAddress);
    scope.destinationAddresses.insert(destinationAddress);
    scope.sourcePorts.set(sourcePort);
    scope.destinationPorts.set(destinationPort);
}

void jsonString(std::ostream& output, std::string_view value) {
    output << '"';
    for (unsigned char character : value) {
        switch (character) {
            case '"': output << "\\\""; break;
            case '\\': output << "\\\\"; break;
            case '\b': output << "\\b"; break;
            case '\f': output << "\\f"; break;
            case '\n': output << "\\n"; break;
            case '\r': output << "\\r"; break;
            case '\t': output << "\\t"; break;
            default:
                if (character < 0x20) throw std::runtime_error("control character in string");
                output << character;
        }
    }
    output << '"';
}

void stringSetJson(std::ostream& output, const std::unordered_set<std::string>& values) {
    std::vector<std::string_view> sorted;
    sorted.reserve(values.size());
    for (const std::string& value : values) sorted.emplace_back(value);
    std::sort(sorted.begin(), sorted.end());
    output << '[';
    for (size_t index = 0; index < sorted.size(); ++index) {
        if (index) output << ',';
        jsonString(output, sorted[index]);
    }
    output << ']';
}

std::string bitmapHex(const std::bitset<65536>& bitmap) {
    static constexpr char digits[] = "0123456789abcdef";
    std::string result;
    result.reserve(16384);
    bool started = false;
    for (int nibble = 16383; nibble >= 0; --nibble) {
        unsigned value = 0;
        for (int bit = 0; bit < 4; ++bit) {
            if (bitmap.test(static_cast<size_t>(nibble * 4 + bit))) value |= 1U << bit;
        }
        if (value || started) {
            result.push_back(digits[value]);
            started = true;
        }
    }
    return started ? result : "0";
}

void scopeJson(std::ostream& output, const Scope& scope) {
    output << "{\"ip_version\":" << scope.ipVersion << ",\"src_visibility\":";
    jsonString(output, scope.srcVisibility);
    output << ",\"dst_visibility\":";
    jsonString(output, scope.dstVisibility);
    output << ",\"metrics\":[";
    for (size_t index = 0; index < scope.metrics.size(); ++index) {
        if (index) output << ',';
        output << scope.metrics[index];
    }
    output << "],\"protocols\":[";
    bool first = true;
    std::vector<std::string> protocols;
    for (size_t protocol = 0; protocol < 256; ++protocol) {
        if (scope.protocols.test(protocol)) protocols.push_back(std::to_string(protocol));
    }
    std::sort(protocols.begin(), protocols.end());
    for (const std::string& protocol : protocols) {
        if (!first) output << ',';
        jsonString(output, protocol);
        first = false;
    }
    output << "],\"source_addresses\":";
    stringSetJson(output, scope.sourceAddresses);
    output << ",\"destination_addresses\":";
    stringSetJson(output, scope.destinationAddresses);
    output << ",\"source_ports_hex\":";
    jsonString(output, bitmapHex(scope.sourcePorts));
    output << ",\"destination_ports_hex\":";
    jsonString(output, bitmapHex(scope.destinationPorts));
    output << '}';
}

std::array<Scope, kScopeCount> makeScopes() {
    std::array<Scope, kScopeCount> scopes;
    static constexpr std::array<std::array<const char*, 2>, 5> visibility{{
        {{"all", "all"}},
        {{"anonymized", "anonymized"}},
        {{"anonymized", "literal"}},
        {{"literal", "anonymized"}},
        {{"literal", "literal"}},
    }};
    for (size_t family = 0; family < 2; ++family) {
        for (size_t index = 0; index < 5; ++index) {
            Scope& scope = scopes[family * 5 + index];
            scope.ipVersion = family == 0 ? 4 : 6;
            scope.srcVisibility = visibility[index][0];
            scope.dstVisibility = visibility[index][1];
        }
    }
    return scopes;
}

void reduce(const std::string& sourceVisibility, const std::string& destinationVisibility) {
    auto scopes = makeScopes();
    std::string line;
    uint64_t lineNumber = 0;
    bool sawHeader = false;
    bool sawNoMatch = false;
    while (std::getline(std::cin, line)) {
        ++lineNumber;
        if (!line.empty() && line.back() == '\r') line.pop_back();
        if (!sawHeader) {
            if (line != kCsvHeader) throw std::runtime_error("line 1: unexpected CSV header");
            sawHeader = true;
            continue;
        }
        if (line == "No matching flows") {
            if (lineNumber != 2) throw std::runtime_error("No matching flows must be the only data row");
            sawNoMatch = true;
            continue;
        }
        if (line.empty()) throw std::runtime_error("line " + std::to_string(lineNumber) + ": empty CSV row");
        if (sawNoMatch) throw std::runtime_error("data row follows No matching flows");
        try {
            const auto fields = splitCsvLine(line);
            parseUnixMilliseconds(fields[0], "time_received");
            const int64_t end = parseUnixMilliseconds(fields[1], "time_end");
            const int64_t start = parseUnixMilliseconds(fields[2], "time_start");
            const uint64_t duration = durationMilliseconds(start, end);
            const std::string sourceAddress(fields[3]);
            const std::string destinationAddress(fields[4]);
            const int ipVersion = addressFamily(fields[3]);
            if (addressFamily(fields[4]) != ipVersion) throw std::runtime_error("mixed IP families");
            const uint16_t sourcePort = parsePort(fields[5]);
            const uint16_t destinationPort = parsePort(fields[6]);
            const uint64_t protocol = parseUnsigned(fields[7], "protocol", 255);
            const uint64_t packets = parseUnsigned(fields[8], "packets", kMaxInteger);
            const uint64_t bytes = parseUnsigned(fields[9], "bytes", kMaxInteger);
            const uint64_t sourceTos = parseUnsigned(fields[10], "src_tos", 255);
            parseUnsigned(fields[11], "dst_tos", 255);
            const uint64_t flowCount = parseUnsigned(fields[12], "flow_count", kMaxInteger);
            if (flowCount == 0) throw std::runtime_error("flow_count must be positive");
            const bool hasMinTtl = !fields[13].empty() && fields[13] != "0";
            const bool hasMaxTtl = !fields[14].empty() && fields[14] != "0";
            const uint64_t minTtl = hasMinTtl ? parseUnsigned(fields[13], "min_ttl", 255) : 0;
            const uint64_t maxTtl = hasMaxTtl ? parseUnsigned(fields[14], "max_ttl", 255) : 0;
            if (hasMinTtl && hasMaxTtl && minTtl > maxTtl) throw std::runtime_error("min_ttl exceeds max_ttl");
            if (!allowsVisibility(sourceTos, sourceVisibility, destinationVisibility)) continue;
            const size_t familyBase = ipVersion == 4 ? 0 : 5;
            for (size_t index : {familyBase, familyBase + exactScopeIndex(sourceTos)}) {
                addFlow(scopes[index], protocol, packets, bytes, flowCount, duration, minTtl, maxTtl,
                        hasMinTtl, hasMaxTtl, sourceAddress, destinationAddress, sourcePort, destinationPort);
            }
        } catch (const std::exception& error) {
            throw std::runtime_error("line " + std::to_string(lineNumber) + ": " + error.what());
        }
    }
    if (!sawHeader) throw std::runtime_error("missing CSV header");

    std::cout << "{\"version\":" << kContractVersion
              << ",\"input_contract\":\"" << kInputContract
              << "\",\"output_contract\":\"" << kOutputContract
              << "\",\"scopes\":[";
    for (size_t index = 0; index < scopes.size(); ++index) {
        if (index) std::cout << ',';
        scopeJson(std::cout, scopes[index]);
    }
    std::cout << "]}\n";
    if (!std::cout) throw std::runtime_error("failed to write reducer output");
}

struct Arguments {
    std::string sourceVisibility;
    std::string destinationVisibility;
    std::string inputContract;
    std::string outputContract;
    int contractVersion = 0;
    bool version = false;
};

Arguments parseArguments(int argc, char** argv) {
    Arguments arguments;
    for (int index = 1; index < argc; ++index) {
        const std::string name = argv[index];
        if (name == "--version") {
            if (argc != 2) throw std::runtime_error("--version cannot be combined with other arguments");
            arguments.version = true;
            continue;
        }
        if (name != "--contract-version" && name != "--input-contract" &&
            name != "--output-contract" && name != "--src-visibility" &&
            name != "--dst-visibility") {
            throw std::runtime_error("unknown argument: " + name);
        }
        if (++index >= argc) throw std::runtime_error("missing value for " + name);
        const std::string value = argv[index];
        if (name == "--contract-version") {
            if (arguments.contractVersion != 0) throw std::runtime_error("duplicate argument: " + name);
            const uint64_t parsed = parseUnsigned(value, "contract version", std::numeric_limits<int>::max());
            arguments.contractVersion = static_cast<int>(parsed);
            continue;
        }
        if (name == "--input-contract" || name == "--output-contract") {
            std::string& target = name == "--input-contract"
                ? arguments.inputContract
                : arguments.outputContract;
            if (!target.empty()) throw std::runtime_error("duplicate argument: " + name);
            target = value;
            continue;
        }
        if (value != "literal" && value != "anonymized") {
            throw std::runtime_error(name + " must be literal or anonymized");
        }
        std::string& target = name == "--src-visibility"
            ? arguments.sourceVisibility
            : arguments.destinationVisibility;
        if (!target.empty()) throw std::runtime_error("duplicate argument: " + name);
        target = value;
    }
    if (!arguments.version) {
        if (arguments.contractVersion != kContractVersion) {
            throw std::runtime_error("unsupported or missing contract version");
        }
        if (arguments.inputContract != kInputContract) {
            throw std::runtime_error("unsupported or missing input contract");
        }
        if (arguments.outputContract != kOutputContract) {
            throw std::runtime_error("unsupported or missing output contract");
        }
    }
    return arguments;
}

}  // namespace

int main(int argc, char** argv) {
    std::signal(SIGPIPE, SIG_IGN);
    try {
        const Arguments arguments = parseArguments(argc, argv);
        if (arguments.version) {
            std::cout << "nfdump_reducer " << kContractVersion << ' ' << kInputContract
                      << ' ' << kOutputContract << '\n';
            return std::cout ? 0 : 1;
        }
        reduce(arguments.sourceVisibility, arguments.destinationVisibility);
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "nfdump_reducer failed: " << error.what() << '\n';
        return 1;
    }
}
