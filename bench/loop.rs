fn main() {
    let mut sum: u64 = 0;
    for i in 1..=ITERATIONS_u64 {
        sum = (sum + i) % 1_000_000_007;
    }
    println!("{sum}");
}
