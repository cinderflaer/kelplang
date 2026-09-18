// Writes 20 GiB to a fresh file in 1 MiB chunks, fsyncs it to storage,
// verifies the size, then deletes it.
//
// The last four lines are the machine-readable contract shared by all four
// implementations: BYTES, WORK_NS, FLUSH_NS, DELETE_NS.
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <string>
#include <sys/stat.h>
#include <unistd.h>

static const size_t MIB = 1u << 20;
static const uint64_t TOTAL = 20ull * 1024 * 1024 * 1024;

static uint64_t now_ns() {
    return (uint64_t)std::chrono::duration_cast<std::chrono::nanoseconds>(
               std::chrono::steady_clock::now().time_since_epoch())
        .count();
}

int main(int argc, char** argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: write20gb <path>\n");
        return 2;
    }
    const char* path = argv[1];
    unlink(path);

    std::string chunk(MIB, 'x');
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        perror("open");
        return 1;
    }
    uint64_t work_start = now_ns();
    uint64_t written = 0;
    while (written < TOTAL) {
        ssize_t n = write(fd, chunk.data(), MIB);
        if (n <= 0) {
            perror("write");
            return 1;
        }
        written += (uint64_t)n;
    }
    uint64_t work_ns = now_ns() - work_start;

    uint64_t flush_start = now_ns();
    if (fsync(fd) != 0) {
        perror("fsync");
        return 1;
    }
    uint64_t flush_ns = now_ns() - flush_start;
    close(fd);

    struct stat st;
    if (stat(path, &st) != 0) {
        perror("stat");
        return 1;
    }
    uint64_t delete_start = now_ns();
    if (unlink(path) != 0) {
        perror("unlink");
        return 1;
    }
    uint64_t delete_ns = now_ns() - delete_start;

    printf("BYTES %llu\n", (unsigned long long)st.st_size);
    printf("WORK_NS %llu\n", (unsigned long long)work_ns);
    printf("FLUSH_NS %llu\n", (unsigned long long)flush_ns);
    printf("DELETE_NS %llu\n", (unsigned long long)delete_ns);
    return 0;
}
