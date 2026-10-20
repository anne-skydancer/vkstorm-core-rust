//! Decode the same (optionally truncated) codestream concurrently on many
//! threads and verify every result equals the single-threaded one. Catches
//! shared mutable state in the codec. usage: thread_stress <file> <reduce> <max_bytes|0> <threads> <rounds>
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let data = std::fs::read(&a[1]).unwrap();
    let reduce: u32 = a[2].parse().unwrap(); let max: usize = a[3].parse().unwrap();
    let threads: usize = a[4].parse().unwrap(); let rounds: usize = a[5].parse().unwrap();
    let reference = vkstorm_j2c::decode(&data, max, reduce, 0, 4).unwrap().data;
    let data = std::sync::Arc::new(data); let reference = std::sync::Arc::new(reference);
    let mismatches = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let handles: Vec<_> = (0..threads).map(|_| {
        let (data, reference, mismatches) = (data.clone(), reference.clone(), mismatches.clone());
        std::thread::spawn(move || {
            for _ in 0..rounds {
                let d = vkstorm_j2c::decode(&data, max, reduce, 0, 4).unwrap().data;
                if d != *reference { mismatches.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
            }
        })
    }).collect();
    for h in handles { h.join().unwrap(); }
    let m = mismatches.load(std::sync::atomic::Ordering::Relaxed);
    println!("{} decodes on {} threads: {} mismatches", threads * rounds, threads, m);
    std::process::exit(if m == 0 { 0 } else { 1 });
}
