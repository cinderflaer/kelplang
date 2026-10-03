/* spectral-norm: the Benchmarks Game spectral norm, 10 power iterations. Size = n (default 1000). */
#include <math.h>
#include <stdio.h>
#include <stdlib.h>

static double eval_a(long long i, long long j) {
    return 1.0 / (double)((i + j) * (i + j + 1) / 2 + i + 1);
}

static void mul_av(const double *v, double *out, long long n) {
    for (long long i = 0; i < n; i++) {
        double s = 0.0;
        for (long long j = 0; j < n; j++) s = s + eval_a(i, j) * v[j];
        out[i] = s;
    }
}

static void mul_atv(const double *v, double *out, long long n) {
    for (long long i = 0; i < n; i++) {
        double s = 0.0;
        for (long long j = 0; j < n; j++) s = s + eval_a(j, i) * v[j];
        out[i] = s;
    }
}

static void mul_atav(const double *v, double *out, double *tmp, long long n) {
    mul_av(v, tmp, n);
    mul_atv(tmp, out, n);
}

int main(int argc, char **argv) {
    long long n = argc > 1 ? atoll(argv[1]) : 1000;
    double *u = malloc(n * sizeof(double));
    double *v = malloc(n * sizeof(double));
    double *tmp = malloc(n * sizeof(double));
    for (long long i = 0; i < n; i++) { u[i] = 1.0; v[i] = 0.0; tmp[i] = 0.0; }
    for (int it = 0; it < 10; it++) {
        mul_atav(u, v, tmp, n);
        mul_atav(v, u, tmp, n);
    }
    double vbv = 0.0, vv = 0.0;
    for (long long i = 0; i < n; i++) {
        vbv = vbv + u[i] * v[i];
        vv = vv + v[i] * v[i];
    }
    printf("spectral-norm %lld %.9f\n", n, sqrt(vbv / vv));
    free(u); free(v); free(tmp);
    return 0;
}
