//! 构建脚本。
//!
//! 除了 Tauri 自己的代码生成，这里还做一件事：**扫描 `assets/articles/*.md`，
//! 生成一份素材清单**（`$OUT_DIR/articles_generated.rs`），由 `src/articles.rs` 用
//! `include!` 引入。
//!
//! 好处：加一篇真题只需把 md 丢进 `assets/articles/`，不用改 Rust 代码，
//! 也不会出现「忘了在列表里补一行，结果文章没生效」这种问题。
//!
//! 注：生成的 `include_str!` 用的是**绝对路径**——因为 `include_str!` 的相对路径
//! 是相对「宏所在文件」（也就是 OUT_DIR 里那份生成文件），不是相对 `articles.rs`。

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    tauri_build::build();

    let assets: PathBuf = Path::new("assets/articles").to_path_buf();
    println!("cargo:rerun-if-changed=assets/articles");

    let mut names: Vec<String> = match fs::read_dir(&assets) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".md"))
            .collect(),
        // 目录不存在不算致命错误：单测 / 部分构建场景下可能没有素材
        Err(_) => Vec::new(),
    };
    // 排序保证顺序稳定：否则每次构建的 seed 顺序会变（列表排序会跳）
    names.sort();

    for name in &names {
        println!("cargo:rerun-if-changed=assets/articles/{name}");
    }

    // Windows 的路径分隔符必须转成正斜杠，否则会变成 Rust 字符串里的转义序列
    let dir = fs::canonicalize(&assets)
        .unwrap_or(assets)
        .to_string_lossy()
        .replace('\\', "/");

    let mut code = String::from(
        "/// 由 `build.rs` 扫描 `assets/articles/` 生成（按文件名排序）。\n\
         pub const ARTICLE_FILES: &[&str] = &[\n",
    );
    for name in &names {
        code.push_str(&format!("    include_str!(\"{dir}/{name}\"),\n"));
    }
    code.push_str("];\n");

    let out_dir = std::env::var("OUT_DIR").expect("缺少 OUT_DIR 环境变量");
    fs::write(Path::new(&out_dir).join("articles_generated.rs"), code)
        .expect("写入 articles_generated.rs 失败");
}
