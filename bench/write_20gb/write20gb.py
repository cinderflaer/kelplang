# Writes 20 GiB to a fresh file in 1 MiB chunks, fsyncs it to storage, verifies
# the size, then deletes it.
#
# The last four lines are the machine-readable contract shared by all four
# implementations: BYTES, WORK_NS, FLUSH_NS, DELETE_NS.
import os
import sys
import time

MIB = 1 << 20
TOTAL = 20 * 1024 * 1024 * 1024


def main():
    if len(sys.argv) < 2:
        print("usage: write20gb.py <path>", file=sys.stderr)
        return 2
    path = sys.argv[1]
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass

    chunk = b"x" * MIB
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o644)

    work_start = time.perf_counter_ns()
    written = 0
    while written < TOTAL:
        # os.write may write fewer bytes than asked; count what landed.
        written += os.write(fd, chunk)
    work_ns = time.perf_counter_ns() - work_start

    flush_start = time.perf_counter_ns()
    os.fsync(fd)
    flush_ns = time.perf_counter_ns() - flush_start
    os.close(fd)

    size = os.stat(path).st_size
    delete_start = time.perf_counter_ns()
    os.unlink(path)
    delete_ns = time.perf_counter_ns() - delete_start

    print(f"BYTES {size}")
    print(f"WORK_NS {work_ns}")
    print(f"FLUSH_NS {flush_ns}")
    print(f"DELETE_NS {delete_ns}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
