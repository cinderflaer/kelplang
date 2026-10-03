/* binary-trees: the Benchmarks Game allocation benchmark (min depth 4, stretch tree, long-lived tree).
   Prints the sum of every tree check. Size = max depth (default 16). malloc/free per node. */
#include <stdio.h>
#include <stdlib.h>

typedef struct Node { struct Node *left, *right; } Node;

static Node *make(int d) {
    Node *n = malloc(sizeof(Node));
    if (d == 0) {
        n->left = NULL;
        n->right = NULL;
    } else {
        n->left = make(d - 1);
        n->right = make(d - 1);
    }
    return n;
}

static long long check(const Node *n) {
    if (n->left == NULL) return 1;
    return 1 + check(n->left) + check(n->right);
}

static void drop(Node *n) {
    if (n->left != NULL) {
        drop(n->left);
        drop(n->right);
    }
    free(n);
}

int main(int argc, char **argv) {
    int max_depth = argc > 1 ? atoi(argv[1]) : 16;
    int min_depth = 4;
    long long total = 0;
    Node *stretch = make(max_depth + 1);
    total += check(stretch);
    drop(stretch);
    Node *long_lived = make(max_depth);
    for (int d = min_depth; d <= max_depth; d += 2) {
        int iters = 1 << (max_depth - d + min_depth);
        long long sum = 0;
        for (int i = 0; i < iters; i++) {
            Node *t = make(d);
            sum += check(t);
            drop(t);
        }
        total += sum;
    }
    total += check(long_lived);
    drop(long_lived);
    printf("binary-trees %d %lld\n", max_depth, total);
    return 0;
}
