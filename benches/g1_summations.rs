use criterion::{black_box, Criterion};
use criterion::criterion_group;
use criterion::criterion_main;

use plonky::{G1_GENERATOR_PROJECTIVE, G1ProjectivePoint, pairwise_affine_summation, pairwise_affine_summation_batch_inversion};

fn criterion_benchmark(c: &mut Criterion) {
    let n = 23;

    // We want a scalar with a Hamming weight of 0.5, to simulate the "average case".
    let mut summands = Vec::new();
    let mut current = G1_GENERATOR_PROJECTIVE;
    for i in 0..n {
        summands.push(current);
        current = current.double();
    }

    let summands = G1ProjectivePoint::batch_to_affine(&summands);

    {
        let summands = summands.clone();
        c.bench_function("G1 pairwise affine summation", move |b| b.iter(|| {
            pairwise_affine_summation(black_box(summands.clone()));
        }));
    }

    {
        let summands = summands.clone();
        c.bench_function("G1 pairwise affine summation (batch inversion)", move |b| b.iter(|| {
            pairwise_affine_summation_batch_inversion(black_box(summands.clone()));
        }));
    }
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);