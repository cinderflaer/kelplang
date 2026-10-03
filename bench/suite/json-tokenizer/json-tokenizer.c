/* json-tokenizer: generate a JSON-style text of at least n bytes (records
   {"id":I,"name":"userJ","tags":["a","b\"c"],"score":K.5,"ok":true,"n":null}, one per line), then
   tokenize it with a byte-at-a-time state machine, counting punctuation, strings, numbers and
   literals and hashing the token stream. Generation is included in the timing, in every language.
   Size = n bytes (default 20000000). */
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

static void buf_push_str(Buf *b, const char *s) { buf_push(b, s, strlen(s)); }

static void buf_push_num(Buf *b, long long v) {
    char tmp[24], out[24];
    size_t n = 0, m = 0;
    if (v == 0) tmp[n++] = '0';
    while (v > 0) { tmp[n++] = (char)('0' + v % 10); v /= 10; }
    while (n > 0) out[m++] = tmp[--n];
    buf_push(b, out, m);
}

static int is_digit(unsigned char c) { return c >= 48 && c <= 57; }
static int is_num_char(unsigned char c) {
    return (c >= 48 && c <= 57) || c == 46 || c == 101 || c == 69 || c == 43 || c == 45;
}
static int is_alpha(unsigned char c) { return c >= 97 && c <= 122; }

int main(int argc, char **argv) {
    size_t target = argc > 1 ? (size_t)atoll(argv[1]) : 20000000;
    Buf s = {0, 0, 0};
    for (long long i = 0; s.len < target; i++) {
        buf_push_str(&s, "{\"id\":");
        buf_push_num(&s, i);
        buf_push_str(&s, ",\"name\":\"user");
        buf_push_num(&s, i % 1000);
        buf_push_str(&s, "\",\"tags\":[\"a\",\"b\\\"c\"],\"score\":");
        buf_push_num(&s, i % 97);
        buf_push_str(&s, ".5,\"ok\":true,\"n\":null},\n");
    }
    size_t text_len = s.len;
    const unsigned char *t = (const unsigned char *)s.data;
    long long punct = 0, strings = 0, numbers = 0, literals = 0, errors = 0, str_bytes = 0;
    uint64_t h = 0;
    size_t p = 0;
    while (p < text_len) {
        unsigned char ch = t[p];
        if (ch == 32 || ch == 10 || ch == 13 || ch == 9) {
            p++;
        } else if (ch == 123 || ch == 125 || ch == 91 || ch == 93 || ch == 58 || ch == 44) {
            punct++;
            h = h * 31 + ch;
            p++;
        } else if (ch == 34) {
            p++;
            size_t start = p;
            while (p < text_len && t[p] != 34) {
                if (t[p] == 92) p++;
                p++;
            }
            strings++;
            str_bytes += (long long)(p - start);
            h = h * 31 + 34;
            p++;
        } else if (is_digit(ch) || ch == 45) {
            while (p < text_len && is_num_char(t[p])) p++;
            numbers++;
            h = h * 31 + 48;
        } else if (is_alpha(ch)) {
            size_t start = p;
            while (p < text_len && is_alpha(t[p])) p++;
            literals++;
            h = h * 31 + (uint64_t)(p - start);
        } else {
            errors++;
            p++;
        }
    }
    printf("json-tokenizer %zu %zu %lld %lld %lld %lld %lld %lld %llu\n", target, text_len, punct, strings,
           str_bytes, numbers, literals, errors, (unsigned long long)h);
    free(s.data);
    return 0;
}
