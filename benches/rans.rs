use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use zvcr::io::serialize::experimental::coders::rans;

const SIZE: usize = 16 * 1024 * 1024;

fn make_data(size: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(size);
    let mut state: u64 = 1;
    for _ in 0..size {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        data.push((state & 0xff) as u8);
    }
    data
}

fn bench_encode(c: &mut Criterion) {
    let data = make_data(SIZE);
    let (freq, start) = rans::build_freq_table(&data);
    let mut group = c.benchmark_group("rans");
    group.throughput(Throughput::Bytes(SIZE as u64));
    group.bench_function("encode", |b| {
        b.iter(|| {
            let mut enc = rans::RansEncoder::new();
            for &b in data.iter().rev() {
                enc.put(freq[b as usize] as u32, start[b as usize] as u32);
            }
            black_box(enc.finish());
        });
    });
    group.finish();
}

fn bench_decode(c: &mut Criterion) {
    let data = make_data(SIZE);
    let (freq, start) = rans::build_freq_table(&data);
    let table = rans::build_decode_table(&freq, &start);
    let mut enc = rans::RansEncoder::new();
    for &b in data.iter().rev() {
        enc.put(freq[b as usize] as u32, start[b as usize] as u32);
    }
    let body = enc.finish();
    let mut group = c.benchmark_group("rans");
    group.throughput(Throughput::Bytes(SIZE as u64));
    group.bench_function("decode", |b| {
        let mut out = vec![0u8; SIZE];
        b.iter(|| {
            let mut dec = rans::RansDecoder::new(black_box(&body));
            for i in 0..SIZE {
                let e = unsafe { table.get_unchecked(dec.slot() as usize) };
                out[i] = e.sym;
                dec.advance(e.freq as u32, e.start as u32);
            }
            black_box(&out);
        });
    });
    group.finish();
}

criterion_group!(benches, bench_encode, bench_decode);
criterion_main!(benches);