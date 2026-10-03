// nbody: the Benchmarks Game n-body simulation, 5 bodies. Size = steps (default 5000000).
#[derive(Clone, Copy)]
struct Body {
    x: f64,
    y: f64,
    z: f64,
    vx: f64,
    vy: f64,
    vz: f64,
    mass: f64,
}

const PI: f64 = 3.141592653589793;
const SOLAR_MASS: f64 = 4.0 * PI * PI;
const DAYS: f64 = 365.24;

fn initial() -> [Body; 5] {
    [
        Body { x: 0.0, y: 0.0, z: 0.0, vx: 0.0, vy: 0.0, vz: 0.0, mass: SOLAR_MASS },
        Body {
            x: 4.84143144246472090,
            y: -1.16032004402742839,
            z: -0.103622044471123109,
            vx: 0.00166007664274403694 * DAYS,
            vy: 0.00769901118419740425 * DAYS,
            vz: -0.0000690460016972063023 * DAYS,
            mass: 0.000954791938424326609 * SOLAR_MASS,
        },
        Body {
            x: 8.34336671824457987,
            y: 4.12479856412430479,
            z: -0.403523417114321381,
            vx: -0.00276742510726862411 * DAYS,
            vy: 0.00499852801234917238 * DAYS,
            vz: 0.0000230417297573763929 * DAYS,
            mass: 0.000285885980666130812 * SOLAR_MASS,
        },
        Body {
            x: 12.8943695621391310,
            y: -15.1111514016986312,
            z: -0.223307578892655734,
            vx: 0.00296460137564761618 * DAYS,
            vy: 0.00237847173959480950 * DAYS,
            vz: -0.0000296589568540237556 * DAYS,
            mass: 0.0000436624404335156298 * SOLAR_MASS,
        },
        Body {
            x: 15.3796971148509165,
            y: -25.9193146099879641,
            z: 0.179258772950371181,
            vx: 0.00268067772490389322 * DAYS,
            vy: 0.00162824170038242295 * DAYS,
            vz: -0.0000951592254519715870 * DAYS,
            mass: 0.0000515138902046611451 * SOLAR_MASS,
        },
    ]
}

fn offset_momentum(b: &mut [Body; 5]) {
    let (mut px, mut py, mut pz) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..5 {
        px += b[i].vx * b[i].mass;
        py += b[i].vy * b[i].mass;
        pz += b[i].vz * b[i].mass;
    }
    b[0].vx = 0.0 - px / SOLAR_MASS;
    b[0].vy = 0.0 - py / SOLAR_MASS;
    b[0].vz = 0.0 - pz / SOLAR_MASS;
}

fn advance(b: &mut [Body; 5], dt: f64) {
    for i in 0..5 {
        for j in (i + 1)..5 {
            let dx = b[i].x - b[j].x;
            let dy = b[i].y - b[j].y;
            let dz = b[i].z - b[j].z;
            let d2 = dx * dx + dy * dy + dz * dz;
            let mag = dt / (d2 * d2.sqrt());
            b[i].vx = b[i].vx - dx * b[j].mass * mag;
            b[i].vy = b[i].vy - dy * b[j].mass * mag;
            b[i].vz = b[i].vz - dz * b[j].mass * mag;
            b[j].vx = b[j].vx + dx * b[i].mass * mag;
            b[j].vy = b[j].vy + dy * b[i].mass * mag;
            b[j].vz = b[j].vz + dz * b[i].mass * mag;
        }
    }
    for i in 0..5 {
        b[i].x = b[i].x + dt * b[i].vx;
        b[i].y = b[i].y + dt * b[i].vy;
        b[i].z = b[i].z + dt * b[i].vz;
    }
}

fn energy(b: &[Body; 5]) -> f64 {
    let mut e = 0.0f64;
    for i in 0..5 {
        e = e + 0.5 * b[i].mass * (b[i].vx * b[i].vx + b[i].vy * b[i].vy + b[i].vz * b[i].vz);
        for j in (i + 1)..5 {
            let dx = b[i].x - b[j].x;
            let dy = b[i].y - b[j].y;
            let dz = b[i].z - b[j].z;
            e = e - b[i].mass * b[j].mass / (dx * dx + dy * dy + dz * dz).sqrt();
        }
    }
    e
}

fn main() {
    let steps: i64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(5000000);
    let mut bodies = initial();
    offset_momentum(&mut bodies);
    let e0 = energy(&bodies);
    for _ in 0..steps {
        advance(&mut bodies, 0.01);
    }
    let e1 = energy(&bodies);
    println!("nbody {} {:.9} {:.9}", steps, e0, e1);
}
