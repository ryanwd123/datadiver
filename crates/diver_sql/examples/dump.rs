//! Prints the syntax tree of the SQL given as the first argument.

fn main() {
    let text = std::env::args().nth(1).unwrap_or_default();
    print!("{}", diver_sql::parse(&text).debug_tree());
}
