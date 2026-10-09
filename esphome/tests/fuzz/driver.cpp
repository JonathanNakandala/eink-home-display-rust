// A small coverage-guided fuzzer for the harnesses in this directory, for machines whose compiler has no libFuzzer
// (Apple's clang has none). A harness is written against libFuzzer's entry point, `LLVMFuzzerTestOneInput`, so with
// libFuzzer
// (`clang++ -fsanitize=fuzzer,address,undefined harness.cpp`) the harness is used as it is and this file is left out.
//
// It keeps a corpus, mutates it, runs each input under the address and undefined-behaviour sanitizers, and keeps an
// input when it reaches code no earlier one did (clang's inline 8-bit counters, bucketed by how often an edge was hit).
// An input that makes the program die is written to the output directory first.
//
//   fuzz_<target> [-seconds=N] [-seed=N] [-max_len=N] [-out=DIR] <file or directory>...
//
// With no `-seconds` the inputs given are each run once and nothing more: a replay, which is how the corpus and the
// regressions are checked quickly. With `-seconds` they are the seeds, and the fuzzer runs for that long.
#include <csignal>
#include <sys/stat.h>

#include <algorithm>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <dirent.h>
#include <fstream>
#include <iterator>
#include <string>
#include <vector>

#include <sanitizer/common_interface_defs.h>

extern "C" int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size);

namespace {

using Input = std::vector<uint8_t>;

uint8_t *counters = nullptr;
size_t counter_count = 0;
std::vector<uint8_t> seen;  // per counter: the buckets of hit counts reached so far

Input current;
std::string out_dir = "build/fuzz-out";
std::string target = "fuzz";

// Which of eight buckets a hit count falls in, so that going round a loop twice and a hundred times differ.
uint8_t bucket(uint8_t hits) {
  if (hits == 0)
    return 0;
  if (hits <= 3)
    return static_cast<uint8_t>(1u << (hits - 1));  // 1, 2, 3 -> bits 0, 1, 2
  if (hits <= 7)
    return 8;
  if (hits <= 15)
    return 16;
  if (hits <= 31)
    return 32;
  if (hits <= 127)
    return 64;
  return 128;
}

// True if the last run reached something new, and remembers it.
bool new_coverage() {
  bool fresh = false;
  for (size_t i = 0; i < counter_count; i++) {
    const uint8_t b = bucket(counters[i]);
    if (b & ~seen[i]) {
      seen[i] = static_cast<uint8_t>(seen[i] | b);
      fresh = true;
    }
  }
  return fresh;
}

size_t edges_reached() {
  size_t n = 0;
  for (size_t i = 0; i < counter_count; i++)
    n += seen[i] != 0;
  return n;
}

void run(const Input &input) {
  current = input;
  if (counters)
    std::memset(counters, 0, counter_count);
  LLVMFuzzerTestOneInput(input.data(), input.size());
}

// Called by the sanitizers as the program dies: the input that did it is saved before it is lost.
void save_crash() {
  mkdir(out_dir.c_str(), 0755);
  uint32_t hash = 2166136261u;
  for (uint8_t b : current)
    hash = (hash ^ b) * 16777619u;
  char name[512];
  std::snprintf(name, sizeof name, "%s/crash-%s-%08x", out_dir.c_str(), target.c_str(), hash);
  std::ofstream file(name, std::ios::binary);
  file.write(reinterpret_cast<const char *>(current.data()), static_cast<std::streamsize>(current.size()));
  std::fprintf(stderr, "fuzz: the input that did it (%zu bytes) is in %s\n", current.size(), name);
}

// A failed check in a harness ends in abort(), which the sanitizers' own callback does not see.
void on_abort(int) {
  save_crash();
  std::signal(SIGABRT, SIG_DFL);
  std::raise(SIGABRT);
}

uint64_t rng_state = 88172645463325252ull;
uint64_t rnd() {
  rng_state ^= rng_state << 13;
  rng_state ^= rng_state >> 7;
  rng_state ^= rng_state << 17;
  return rng_state;
}
size_t below(size_t n) { return n == 0 ? 0 : static_cast<size_t>(rnd() % n); }

const uint8_t INTERESTING[] = {0x00, 0x01, 0x7f, 0x80, 0xff, '\r', '\n', ' ',  ':',
                               ';',  '=',  '-',  '0',  '9',  'f',  'F',  0x30, 0x82};

void mutate(Input &in, const std::vector<Input> &corpus, size_t max_len) {
  const int rounds = 1 + static_cast<int>(below(4));
  for (int r = 0; r < rounds; r++) {
    switch (below(9)) {
      case 0:  // flip a bit
        if (!in.empty())
          in[below(in.size())] ^= static_cast<uint8_t>(1u << below(8));
        break;
      case 1:  // set a byte
        if (!in.empty())
          in[below(in.size())] = static_cast<uint8_t>(rnd());
        break;
      case 2:  // set an interesting byte
        if (!in.empty())
          in[below(in.size())] = INTERESTING[below(sizeof INTERESTING)];
        break;
      case 3:  // insert bytes
        if (in.size() < max_len) {
          const size_t at = below(in.size() + 1), n = 1 + below(8);
          Input add(n);
          for (auto &b : add)
            b = rnd() % 4 == 0 ? INTERESTING[below(sizeof INTERESTING)] : static_cast<uint8_t>(rnd());
          in.insert(in.begin() + static_cast<long>(at), add.begin(), add.end());
        }
        break;
      case 4:  // delete bytes
        if (!in.empty()) {
          const size_t at = below(in.size()), n = 1 + below(std::min<size_t>(8, in.size() - at));
          in.erase(in.begin() + static_cast<long>(at), in.begin() + static_cast<long>(at + n));
        }
        break;
      case 5:  // repeat a piece
        if (!in.empty() && in.size() < max_len) {
          const size_t at = below(in.size()), n = 1 + below(std::min<size_t>(16, in.size() - at));
          const Input piece(in.begin() + static_cast<long>(at), in.begin() + static_cast<long>(at + n));
          in.insert(in.begin() + static_cast<long>(at), piece.begin(), piece.end());
        }
        break;
      case 6:  // add to or take from a byte
        if (!in.empty())
          in[below(in.size())] = static_cast<uint8_t>(in[below(in.size())] + (rnd() % 2 ? 1 : -1));
        break;
      case 7:  // take a piece of another input
        if (!corpus.empty()) {
          const Input &other = corpus[below(corpus.size())];
          if (!other.empty() && in.size() < max_len) {
            const size_t from = below(other.size()), n = 1 + below(std::min<size_t>(32, other.size() - from));
            in.insert(in.begin() + static_cast<long>(below(in.size() + 1)), other.begin() + static_cast<long>(from),
                      other.begin() + static_cast<long>(from + n));
          }
        }
        break;
      default:  // a run of one byte
        if (!in.empty()) {
          const size_t at = below(in.size()), n = 1 + below(std::min<size_t>(32, in.size() - at));
          std::fill(in.begin() + static_cast<long>(at), in.begin() + static_cast<long>(at + n), in[at]);
        }
        break;
    }
  }
  if (in.size() > max_len)
    in.resize(max_len);
}

void load(const std::string &path, std::vector<Input> &into) {
  struct stat info;
  if (stat(path.c_str(), &info) != 0)
    return;
  if (S_ISDIR(info.st_mode)) {
    if (DIR *dir = opendir(path.c_str())) {
      std::vector<std::string> names;
      while (const dirent *entry = readdir(dir))
        if (entry->d_name[0] != '.')
          names.push_back(path + "/" + entry->d_name);
      closedir(dir);
      std::sort(names.begin(), names.end());
      for (const auto &name : names)
        load(name, into);
    }
    return;
  }
  std::ifstream file(path, std::ios::binary);
  into.emplace_back(std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>());
}

}  // namespace

// Called as the instrumented code starts up, before this file's own globals are built (the order between files is not
// defined), so it only notes where the counters are; `main` sets up the rest.
extern "C" void __sanitizer_cov_8bit_counters_init(uint8_t *start, uint8_t *stop) {
  counters = start;
  counter_count = static_cast<size_t>(stop - start);
}

int main(int argc, char **argv) {
  double seconds = 0;
  size_t max_len = 4096;
  std::vector<std::string> paths;
  for (int i = 1; i < argc; i++) {
    const std::string arg = argv[i];
    if (arg.rfind("-seconds=", 0) == 0)
      seconds = std::atof(arg.c_str() + 9);
    else if (arg.rfind("-seed=", 0) == 0)
      rng_state ^= std::strtoull(arg.c_str() + 6, nullptr, 10) * 0x9E3779B97F4A7C15ull;
    else if (arg.rfind("-max_len=", 0) == 0)
      max_len = static_cast<size_t>(std::atol(arg.c_str() + 9));
    else if (arg.rfind("-out=", 0) == 0)
      out_dir = arg.substr(5);
    else
      paths.push_back(arg);
  }
  const std::string self = argv[0];
  target = self.substr(self.find_last_of('/') == std::string::npos ? 0 : self.find_last_of('/') + 1);
  __sanitizer_set_death_callback(save_crash);
  std::signal(SIGABRT, on_abort);
  seen.assign(counter_count, 0);

  std::vector<Input> corpus;
  for (const auto &path : paths)
    load(path, corpus);
  if (corpus.empty())
    corpus.push_back(Input());

  uint64_t executions = 0;
  // The seeds first: run once each, and keep what they reach.
  std::vector<Input> kept;
  for (const Input &seed : corpus) {
    run(seed);
    executions++;
    if (new_coverage() || kept.empty())
      kept.push_back(seed);
  }
  if (seconds <= 0) {
    std::printf("%s: replayed %zu inputs, %zu edges\n", target.c_str(), corpus.size(), edges_reached());
    return 0;
  }

  const auto until = std::chrono::steady_clock::now() + std::chrono::milliseconds(static_cast<long>(seconds * 1000));
  while (std::chrono::steady_clock::now() < until) {
    for (int batch = 0; batch < 256; batch++) {
      Input candidate = kept[below(kept.size())];
      mutate(candidate, kept, max_len);
      run(candidate);
      executions++;
      if (new_coverage())
        kept.push_back(candidate);
    }
  }
  std::printf("%s: %llu runs in %.0f s, corpus %zu, %zu edges\n", target.c_str(),
              static_cast<unsigned long long>(executions), seconds, kept.size(), edges_reached());
  // The corpus that grew is saved, for the next run to start from.
  mkdir(out_dir.c_str(), 0755);
  const std::string grown = out_dir + "/corpus-" + target;
  mkdir(grown.c_str(), 0755);
  for (size_t i = 0; i < kept.size(); i++) {
    char name[600];
    std::snprintf(name, sizeof name, "%s/%06zu", grown.c_str(), i);
    std::ofstream file(name, std::ios::binary);
    file.write(reinterpret_cast<const char *>(kept[i].data()), static_cast<std::streamsize>(kept[i].size()));
  }
  return 0;
}
