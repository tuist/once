fn main() {
    println!("{}", hyperfine::summary(3));
}

pub mod hyperfine {
    pub fn summary(runs: u32) -> String {
        format!("{runs} runs")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn summarizes_runs() {
        assert_eq!(super::hyperfine::summary(3), "3 runs");
    }
}
