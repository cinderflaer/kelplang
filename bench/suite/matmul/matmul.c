/* matmul: C = A * B for n x n f64 matrices, naive i-j-k loops over row-major flat storage.
   Prints the sum of all elements of C and its trace. Size = n (default 400). */
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    long long n = argc > 1 ? atoll(argv[1]) : 400;
    size_t nu = (size_t)n;
    double *a = malloc(nu * nu * sizeof(double));
    double *b = malloc(nu * nu * sizeof(double));
    double *c = malloc(nu * nu * sizeof(double));
    for (size_t i = 0; i < nu; i++)
        for (size_t j = 0; j < nu; j++) {
            a[i * nu + j] = (double)((i + j) % 7) * 0.25;
            b[i * nu + j] = (double)((i * 3 + j) % 5) * 0.5;
            c[i * nu + j] = 0.0;
        }
    for (size_t i = 0; i < nu; i++)
        for (size_t j = 0; j < nu; j++) {
            double s = 0.0;
            for (size_t k = 0; k < nu; k++) s = s + a[i * nu + k] * b[k * nu + j];
            c[i * nu + j] = s;
        }
    double sum = 0.0, trace = 0.0;
    for (size_t i = 0; i < nu; i++) {
        for (size_t j = 0; j < nu; j++) sum = sum + c[i * nu + j];
        trace = trace + c[i * nu + i];
    }
    printf("matmul %lld %.3f %.3f\n", n, sum, trace);
    free(a); free(b); free(c);
    return 0;
}
