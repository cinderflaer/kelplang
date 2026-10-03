/* strings: build one string of n comma-separated pieces ("p<i>,"), split it into n owned strings,
   then fold every byte of every piece into a hash. Size = n pieces (default 1000000).
   Growable buffer by hand; every piece is a separate malloc'd copy (like Kelp's owned List<String>). */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct { char *data; size_t len, cap; } Buf;

static void buf_push(Buf *b, const char *s, size_t n) {
    if (b->len + n + 1 > b->cap) {
        size_t nc = b->cap ? b->cap * 2 : 16;
        while (nc < b->len + n + 1) nc *= 2;
        b->data = realloc(b->data, nc);
        b->cap = nc;
    }
    memcpy(b->data + b->len, s, n);
    b->len += n;
    b->data[b->len] = 0;
}

static size_t itoa_ll(long long v, char *out) {
    char tmp[24];
    size_t n = 0, m = 0;
    if (v == 0) tmp[n++] = '0';
    while (v > 0) { tmp[n++] = (char)('0' + v % 10); v /= 10; }
    while (n > 0) out[m++] = tmp[--n];
    return m;
}

int main(int argc, char **argv) {
    long long n = argc > 1 ? atoll(argv[1]) : 1000000;
    Buf s = {0, 0, 0};
    char num[24];
    for (long long i = 0; i < n; i++) {
        buf_push(&s, "p", 1);
        size_t m = itoa_ll(i, num);
        buf_push(&s, num, m);
        buf_push(&s, ",", 1);
    }
    size_t text_len = s.len;
    size_t np = 0, pcap = 16;
    char **pieces = malloc(pcap * sizeof(char *));
    size_t start = 0;
    for (size_t i = 0; i <= s.len; i++) {
        if (i == s.len || s.data[i] == ',') {
            size_t l = i - start;
            char *p = malloc(l + 1);
            memcpy(p, s.data + start, l);
            p[l] = 0;
            if (np == pcap) { pcap *= 2; pieces = realloc(pieces, pcap * sizeof(char *)); }
            pieces[np++] = p;
            start = i + 1;
        }
    }
    long long count = 0, total_len = 0;
    uint64_t h = 0;
    for (size_t k = 0; k < np; k++) {
        const char *p = pieces[k];
        size_t len = strlen(p);
        if (len > 0) {
            count++;
            total_len += (long long)len;
            for (size_t b = 0; b < len; b++) h = h * 31 + (uint64_t)(unsigned char)p[b];
        }
    }
    printf("strings %lld %zu %lld %lld %llu\n", n, text_len, count, total_len, (unsigned long long)h);
    for (size_t k = 0; k < np; k++) free(pieces[k]);
    free(pieces);
    free(s.data);
    return 0;
}
