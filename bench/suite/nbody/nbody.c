/* nbody: the Benchmarks Game n-body simulation, 5 bodies. Size = steps (default 5000000). */
#include <math.h>
#include <stdio.h>
#include <stdlib.h>

typedef struct { double x, y, z, vx, vy, vz, mass; } Body;

#define PI 3.141592653589793
#define SOLAR_MASS (4.0 * PI * PI)
#define DAYS 365.24

static Body bodies[5] = {
    {0.0, 0.0, 0.0, 0.0, 0.0, 0.0, SOLAR_MASS},
    {4.84143144246472090, -1.16032004402742839, -0.103622044471123109,
     0.00166007664274403694 * DAYS, 0.00769901118419740425 * DAYS,
     -0.0000690460016972063023 * DAYS, 0.000954791938424326609 * SOLAR_MASS},
    {8.34336671824457987, 4.12479856412430479, -0.403523417114321381,
     -0.00276742510726862411 * DAYS, 0.00499852801234917238 * DAYS,
     0.0000230417297573763929 * DAYS, 0.000285885980666130812 * SOLAR_MASS},
    {12.8943695621391310, -15.1111514016986312, -0.223307578892655734,
     0.00296460137564761618 * DAYS, 0.00237847173959480950 * DAYS,
     -0.0000296589568540237556 * DAYS, 0.0000436624404335156298 * SOLAR_MASS},
    {15.3796971148509165, -25.9193146099879641, 0.179258772950371181,
     0.00268067772490389322 * DAYS, 0.00162824170038242295 * DAYS,
     -0.0000951592254519715870 * DAYS, 0.0000515138902046611451 * SOLAR_MASS},
};

static void offset_momentum(Body *b) {
    double px = 0.0, py = 0.0, pz = 0.0;
    for (int i = 0; i < 5; i++) {
        px += b[i].vx * b[i].mass;
        py += b[i].vy * b[i].mass;
        pz += b[i].vz * b[i].mass;
    }
    b[0].vx = 0.0 - px / SOLAR_MASS;
    b[0].vy = 0.0 - py / SOLAR_MASS;
    b[0].vz = 0.0 - pz / SOLAR_MASS;
}

static void advance(Body *b, double dt) {
    for (int i = 0; i < 5; i++) {
        for (int j = i + 1; j < 5; j++) {
            double dx = b[i].x - b[j].x;
            double dy = b[i].y - b[j].y;
            double dz = b[i].z - b[j].z;
            double d2 = dx * dx + dy * dy + dz * dz;
            double mag = dt / (d2 * sqrt(d2));
            b[i].vx = b[i].vx - dx * b[j].mass * mag;
            b[i].vy = b[i].vy - dy * b[j].mass * mag;
            b[i].vz = b[i].vz - dz * b[j].mass * mag;
            b[j].vx = b[j].vx + dx * b[i].mass * mag;
            b[j].vy = b[j].vy + dy * b[i].mass * mag;
            b[j].vz = b[j].vz + dz * b[i].mass * mag;
        }
    }
    for (int i = 0; i < 5; i++) {
        b[i].x = b[i].x + dt * b[i].vx;
        b[i].y = b[i].y + dt * b[i].vy;
        b[i].z = b[i].z + dt * b[i].vz;
    }
}

static double energy(const Body *b) {
    double e = 0.0;
    for (int i = 0; i < 5; i++) {
        e = e + 0.5 * b[i].mass * (b[i].vx * b[i].vx + b[i].vy * b[i].vy + b[i].vz * b[i].vz);
        for (int j = i + 1; j < 5; j++) {
            double dx = b[i].x - b[j].x;
            double dy = b[i].y - b[j].y;
            double dz = b[i].z - b[j].z;
            e = e - b[i].mass * b[j].mass / sqrt(dx * dx + dy * dy + dz * dz);
        }
    }
    return e;
}

int main(int argc, char **argv) {
    long long steps = argc > 1 ? atoll(argv[1]) : 5000000;
    offset_momentum(bodies);
    double e0 = energy(bodies);
    for (long long s = 0; s < steps; s++) advance(bodies, 0.01);
    double e1 = energy(bodies);
    printf("nbody %lld %.9f %.9f\n", steps, e0, e1);
    return 0;
}
