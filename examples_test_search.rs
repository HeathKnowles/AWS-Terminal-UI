fn main() {
    let data_dir = aws_tui::loader::default_data_dir().unwrap();
    let tree = aws_tui::loader::load_services(&data_dir, "2.36.18");
    let idx = aws_tui::index::SearchIndex::build(&tree);
    for q in ["ec2 describe-security-groups", "describe-instances"] {
        let res = idx.search(q, 8);
        println!("query: {q}");
        for (i, s) in res {
            println!("  {} score={}", idx.rows[i].haystack, s);
        }
    }
}
