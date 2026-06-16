fn main() {
    let mut out = itoa::Buffer::new();
    println!("{}", out.format(42));
}
