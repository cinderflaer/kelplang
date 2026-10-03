/* mandelbrot: escape-time count over an n x n grid of [-1.5,0.5] x [-1,1], 50 iterations max.
   Prints the number of points that never escape and the total iteration count. Size = n (default 1000). */
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    long long n = argc > 1 ? atoll(argv[1]) : 1000;
    double nf = (double)n;
    long long inside = 0, total = 0;
    for (long long py = 0; py < n; py++) {
        double ci = 2.0 * (double)py / nf - 1.0;
        for (long long px = 0; px < n; px++) {
            double cr = 2.0 * (double)px / nf - 1.5;
            double zr = 0.0, zi = 0.0;
            long long it = 0;
            while (it < 50 && zr * zr + zi * zi <= 4.0) {
                double t = zr * zr - zi * zi + cr;
                zi = 2.0 * zr * zi + ci;
                zr = t;
                it++;
            }
            total += it;
            if (it == 50) inside++;
        }
    }
    printf("mandelbrot %lld %lld %lld\n", n, inside, total);
    return 0;
}
