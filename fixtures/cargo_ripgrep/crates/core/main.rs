fn main() {
    let pattern = std::env::args().nth(1).unwrap_or_default();
    println!("{}", grep::describe(&pattern));
}
