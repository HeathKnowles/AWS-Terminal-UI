fn main() {
    let data_dir = aws_tui::aws::loader::default_data_dir().unwrap();
    let tree = aws_tui::aws::loader::load_services(&data_dir, "2.36.18");
    let idx = aws_tui::aws::index::SearchIndex::build(&tree);
    for q in [
        "ec2 describe-security-groups",
        "ec2 describe-vpcs",
        "describe-instances",
    ] {
        let res = idx.search(q, 8);
        println!("query: {q}");
        for (i, s) in res {
            println!("  {} score={}", idx.rows[i].haystack, s);
        }
    }
}
