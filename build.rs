fn main() {
    let src_dir = "tree-sitter-proverif/src";

    cc::Build::new()
        .include(src_dir)
        .include(format!("{src_dir}/tree_sitter"))
        .file(format!("{src_dir}/parser.c"))
        .file(format!("{src_dir}/scanner.c"))
        .warnings(false)
        .compile("tree-sitter-proverif");

    println!("cargo:rerun-if-changed=tree-sitter-proverif/src/parser.c");
    println!("cargo:rerun-if-changed=tree-sitter-proverif/src/scanner.c");
    println!("cargo:rerun-if-changed=tree-sitter-proverif/src/tree_sitter/parser.h");
}
