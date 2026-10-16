//! 阅读文章：段落切分规则 + 内置四六级素材（纯逻辑，不依赖 Tauri，可在桌面 `cargo test`）。
//!
//! 这一层只负责「文本 → 段落数组」以及内置素材本身，
//! 落库与命令暴露在 `db` / `commands` 里。
//!
//! 段落切分的唯一硬要求：**不能在行尾把单词切断**。
//! 粘贴进来的文本常带 PDF 硬换行（每一行都换行）与 Windows 的 `\r\n`，
//! 直接按 `\n` 渲染就会出现「sixteen-」和「ounce」被拆成两行两个词的情况。

/// 内置文章（自写示例）：id 稳定，正文用空行分段。
pub struct BuiltinArticle {
    pub id: &'static str,
    pub title: &'static str,
    /// 四六级核心词（全部取自正文/标题，有单测锁死这个一致性）
    pub keywords: &'static [&'static str],
    pub body: &'static str,
}

use std::sync::OnceLock;

/// 解析后的素材（真题选文与自写示例统一成这个形状）。
#[derive(Debug, Clone, PartialEq)]
pub struct ArticleSource {
    pub id: String,
    pub title: String,
    /// 出处：哪年真题、选自哪篇外刊（列表与详情页展示）
    pub origin: String,
    pub keywords: Vec<String>,
    pub body: String,
}

// 真题素材清单由 `build.rs` 扫描 `assets/articles/*.md` **自动生成**（见 `src-tauri/build.rs`）：
// 加一篇真题只需要把 md 丢进那个目录，不用改这里，也不会漏加一行。
//
// `include_str!` 在编译期嵌入素材，所以运行时不需要文件系统（APK 里也不用额外打包）。
//
// 版权说明：四六级真题的阅读选文本身改编自外刊，仅供个人背单词使用；
// 每篇的出处逐条写在对应 md 的 front-matter 里。
//
// md 格式：`---` 包围的 front-matter（id / title / source / keywords）+ 空行分段的正文。
// 批量导入用 `node scripts/import-cet-articles.mjs --exam all --year 2022`。
//
// 注：这里用普通注释而非文档注释 —— 文档注释挂在 `include!` 宏上不生效，会报 unused_doc_comments。
include!(concat!(env!("OUT_DIR"), "/articles_generated.rs"));

/// 解析素材 md：front-matter 取字段，剩下的全部当正文。
fn parse_article_md(raw: &str) -> Result<ArticleSource, String> {
    // 有的编辑器会写 BOM，去掉它否则 front-matter 的开头会多一个字符
    let trimmed = raw.trim_start_matches('\u{feff}').trim_start();
    let rest = trimmed
        .strip_prefix("---")
        .ok_or_else(|| "素材缺少 front-matter（应以 --- 开头）".to_string())?;
    let (head, body) = rest
        .split_once("\n---")
        .ok_or_else(|| "素材 front-matter 没有收尾的 ---".to_string())?;

    let mut id = String::new();
    let mut title = String::new();
    let mut origin = String::new();
    let mut keywords: Vec<String> = Vec::new();

    for line in head.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "id" => id = value.to_string(),
            "title" => title = value.to_string(),
            // md 里叫 source（读起来自然），落库时写进 articles.origin
            "source" => origin = value.to_string(),
            "keywords" => {
                keywords = value
                    .split(',')
                    .map(|k| k.trim().to_string())
                    .filter(|k| !k.is_empty())
                    .collect();
            }
            _ => {}
        }
    }

    if id.is_empty() || title.is_empty() {
        return Err("素材缺少 id 或 title".to_string());
    }

    Ok(ArticleSource {
        id,
        title,
        origin,
        keywords,
        body: body.trim().to_string(),
    })
}

/// 全部内置素材：自写示例（内联）+ `assets/articles/` 下的真题选文。
///
/// 顺序即 seed 顺序；`db::seed_builtin_articles` 按 id 增量补插，
/// 所以以后在这里新增文章，已装 App 的用户下次启动也会拿到。
/// 解析结果的缓存：`builtin_articles()` 会被 seed 与「是否内置」判断反复调用，
/// 每次重新解析 6 个 md 是白浪费。
static BUILTIN_CACHE: OnceLock<Vec<ArticleSource>> = OnceLock::new();

pub fn builtin_articles() -> &'static [ArticleSource] {
    BUILTIN_CACHE.get_or_init(|| {
        let mut out: Vec<ArticleSource> = BUILTIN_ARTICLES
            .iter()
            .map(|a| ArticleSource {
                id: a.id.to_string(),
                title: a.title.to_string(),
                origin: "自写示例素材（按四六级高频词编写，非真题原文）".to_string(),
                keywords: a.keywords.iter().map(|k| k.to_string()).collect(),
                body: a.body.to_string(),
            })
            .collect();

        for raw in ARTICLE_FILES {
            match parse_article_md(raw) {
                Ok(article) => out.push(article),
                // 素材是编译期嵌入的，解析失败属于开发期错误：
                // 直接 panic 比静默少一篇更容易被发现（`cargo test` 会立刻拦住）。
                Err(err) => panic!("内置素材解析失败：{err}"),
            }
        }
        out
    })
}

/// 这篇是不是内置素材？（用户删内置文章时要记账，见 `db::delete_article`）
pub fn is_builtin_article(id: &str) -> bool {
    builtin_articles().iter().any(|a| a.id == id)
}

/// 把原始正文规范化，再切成段落。
///
/// 规则（按行扫描）：
/// - `\r\n` / `\r` 统一成 `\n`
/// - 空行（trim 后为空）＝段落边界，连续空行不产生空段
/// - 行尾是 `-` 的换行 = 连字符换行：**只去掉换行，保留连字符**
///   （`sugar-\nfree` → `sugar-free`）
/// - 其它单换行 = PDF 硬换行：换成一个空格接上
/// - 段内连续空白收敛为一个空格
pub fn split_paragraphs(raw: &str) -> Vec<String> {
    let unified = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current = String::new();

    for line in unified.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush(&mut paragraphs, &mut current);
            continue;
        }
        if current.is_empty() {
            current.push_str(trimmed);
        } else if current.ends_with('-') {
            // 连字符换行：软连字符（signifi-/cant）与真连字符（sugar-/free）
            // 无法区分，保留连字符是唯一不丢字符的选择。
            current.push_str(trimmed);
        } else {
            current.push(' ');
            current.push_str(trimmed);
        }
    }
    flush(&mut paragraphs, &mut current);
    paragraphs
}

fn flush(out: &mut Vec<String>, current: &mut String) {
    let text = current.split_whitespace().collect::<Vec<_>>().join(" ");
    if !text.is_empty() {
        out.push(text);
    }
    current.clear();
}

// ---------------------------------------------------------------------------
// 内置素材
// ---------------------------------------------------------------------------
//
// 素材是我们自己写的四六级高频词文章，不复制真题原文（版权）。
// 想读真题原文，用「粘贴导入」自己贴进来即可。

pub const BUILTIN_ARTICLES: &[BuiltinArticle] = &[
    BuiltinArticle {
        id: "builtin-food-label",
        title: "What Food Labels Really Tell Us",
        keywords: &[
            "striking",
            "comparison",
            "inspection",
            "concerns",
            "significant",
            "reveals",
            "vary",
            "enormous",
            "illustrates",
            "dispute",
            "compensate",
            "inaccurate",
            "consume",
            "ingredient",
            "guarantees",
        ],
        body: r#"The figures on a product's label often appear striking. On a company's website, a sixteen-ounce
"Sugar-Free Caramel Protein Matcha" was listed as containing sixteen grams of sugar, while a
twenty-ounce serving contained twenty-one grams. By comparison, a single peanut-butter cup contains
eleven grams. Such numbers suggest that the word "free" on a package deserves closer inspection than
most shoppers give it.

For customers who limit sugar because of general health concerns, diabetes or blood-glucose levels,
the difference between what a product's name suggests and what its nutrition information reveals may
be highly significant. Manufacturers are generally allowed to describe a product as "sugar-free" when
each serving contains less than half a gram of sugar, yet serving sizes vary enormously, and a single
package may hold two or three servings.

The federal lawsuit filed in Seattle on behalf of three consumers illustrates how quickly a marketing
claim can become a legal dispute. The plaintiffs argue that the label misled them into believing the
drink was a healthier alternative, and they demand that the company either correct its packaging or
compensate buyers. The company, for its part, insists that nothing on the label is inaccurate.

Whatever the court decides, the case encourages ordinary readers to develop a habit that serves them
well in every aisle: compare the serving size with the amount you actually consume, read the ingredient
list before the front of the box, and treat adjectives such as "natural" or "light" as invitations to
investigate rather than guarantees."#,
    },
    BuiltinArticle {
        id: "builtin-remote-work",
        title: "Remote Work and the Future of Cities",
        keywords: &[
            "predicted",
            "reshape",
            "regarded",
            "foundation",
            "revenue",
            "economists",
            "converting",
            "vacant",
            "permanent",
            "collapsed",
            "municipal",
            "severe",
            "complicated",
            "deserted",
            "density",
            "efficiency",
            "genuine",
            "deliberate",
            "discipline",
            "thrive",
        ],
        body: r#"When millions of employees began working from home, few predicted how deeply the arrangement
would reshape the places they live. Downtown office towers, once regarded as the foundation of urban
economies, suddenly stood half empty. Restaurants that depended on lunchtime crowds lost the majority
of their revenue, and small shops that had survived for decades announced closures within months.

Economists offer competing explanations for what happens next. One camp maintains that cities will
adapt as they always have, converting vacant offices into apartments and attracting residents who
value short commutes. Another argues that the shift is permanent, that the demand for office space has
collapsed for good, and that municipal budgets dependent on property taxes will face severe pressure.

Evidence so far supports a more complicated picture. In some neighborhoods, remote workers have
strengthened local trade by spending more near home; in others, the loss of commuters has left entire
streets deserted. What seems clear is that the advantages of density — variety, efficiency,
opportunity — remain genuine, but they must now be delivered in new ways.

For individuals, the practical lesson is to avoid assuming that any single trend will determine their
future. A flexible arrangement gives workers more control over their schedule, yet it also requires
discipline, reliable technology and a deliberate separation between professional and private life.
Those who treat the change as an opportunity to design a better routine will probably thrive."#,
    },
    BuiltinArticle {
        id: "builtin-reading-slowly",
        title: "Why Reading Slowly Matters",
        keywords: &[
            "indicator",
            "retain",
            "whereas",
            "essential",
            "comprehension",
            "obstacles",
            "efficient",
            "illusion",
            "grasped",
            "distraction",
            "fragment",
            "concentration",
            "remedies",
            "complicated",
            "summarize",
            "interpretation",
            "deliberate",
            "recall",
            "consistent",
            "analysis",
            "recognition",
            "luxury",
            "economical",
        ],
        body: r#"Students preparing for examinations often measure progress by the number of pages completed, yet
speed is a poor indicator of understanding. A reader who races through a difficult passage may retain
almost nothing, whereas someone who pauses to question each argument will remember the essential
points for years.

Researchers who study comprehension identify several obstacles to efficient reading. The first is the
illusion of fluency: because every word is familiar, we assume we have grasped the meaning, even when
the author's logic has escaped us. The second is distraction — notifications, unfinished tasks and the
temptation to check messages fragment our attention until concentration becomes impossible.

Fortunately, the remedies are neither expensive nor complicated. Read a paragraph, then summarize it
in your own words without looking back. When a sentence resists interpretation, locate its subject and
its main verb before worrying about every modifier. Write brief notes in the margin, and return to them
a day later; the deliberate effort of recall strengthens memory far more than rereading does.

Teachers who adopt this approach report a consistent pattern: students read less in total but perform
better on tests that require analysis rather than recognition. The conclusion is not that reading
should always be slow, but that the pace ought to match the purpose. For material you must master,
patience is not a luxury; it is the most economical strategy available."#,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_paragraphs_on_blank_lines() {
        let raw = "First paragraph.\n\nSecond paragraph.\n\n\nThird paragraph.\n";
        assert_eq!(
            split_paragraphs(raw),
            vec!["First paragraph.", "Second paragraph.", "Third paragraph."]
        );
    }

    #[test]
    fn joins_pdf_hard_wraps_with_a_space() {
        let raw = "The figures on a product's\nlabel often appear striking.\n";
        assert_eq!(
            split_paragraphs(raw),
            vec!["The figures on a product's label often appear striking."]
        );
    }

    /// 成功标准 1 的落点：连字符换行不能把词切断，也不能丢掉连字符。
    #[test]
    fn keeps_hyphenated_line_break_intact() {
        let raw = "a sixteen-ounce Sugar-\nFree Caramel\nMatcha was listed\n";
        assert_eq!(
            split_paragraphs(raw),
            vec!["a sixteen-ounce Sugar-Free Caramel Matcha was listed"]
        );
    }

    #[test]
    fn normalizes_crlf_and_collapses_whitespace() {
        let raw = "Line one\r\nline two\r\n\r\n  Second   paragraph  with\ttabs \r\n";
        assert_eq!(
            split_paragraphs(raw),
            vec!["Line one line two", "Second paragraph with tabs"]
        );
    }

    #[test]
    fn empty_input_yields_no_paragraphs() {
        assert!(split_paragraphs("").is_empty());
        assert!(split_paragraphs("\n\n   \n").is_empty());
    }

    #[test]
    fn builtin_articles_are_well_formed() {
        let articles = builtin_articles();
        // 数量**不写死**：素材会按年份批次持续增加（见 scripts/import-cet-articles.mjs），
        // 这里只兜住下限（防误删素材目录），自写示例则固定 3 篇。
        assert!(
            articles.len() >= 21,
            "内置素材不应少于 21 篇（实际 {}）",
            articles.len()
        );
        assert_eq!(
            articles
                .iter()
                .filter(|a| a.id.starts_with("builtin-"))
                .count(),
            3,
            "自写示例应保持 3 篇"
        );

        for article in articles {
            let paragraphs = split_paragraphs(&article.body);
            assert!(
                paragraphs.len() >= 3,
                "{} 段落数不足 3（实际 {}）",
                article.id,
                paragraphs.len()
            );
            assert!(!article.title.trim().is_empty(), "{} 缺少标题", article.id);
            assert!(!article.origin.trim().is_empty(), "{} 缺少出处", article.id);

            // 标题也算正文：真题标题里常常就有核心词
            let haystack = format!("{} {}", article.title, paragraphs.join(" ")).to_lowercase();
            for keyword in &article.keywords {
                assert!(
                    haystack.contains(&keyword.to_lowercase()),
                    "{} 标注的核心词 `{keyword}` 没出现在正文或标题里",
                    article.id
                );
            }
        }
    }

    /// id 重复会让 seed 互相覆盖（列表少一篇且很难察觉）。
    #[test]
    fn article_ids_are_unique() {
        let mut ids: Vec<String> = builtin_articles().iter().map(|a| a.id.clone()).collect();
        let total = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), total, "内置素材 id 不能重复");
        assert!(is_builtin_article("cet4-2025-12-1-bike-lanes"));
        assert!(!is_builtin_article("import-1"));
    }

    #[test]
    fn parses_front_matter_and_body() {
        let raw =
            "---\nid: x\ntitle: T\nsource: S\nkeywords: a, b\n---\n\nFirst para.\n\nSecond para.\n";
        let article = parse_article_md(raw).unwrap();
        assert_eq!(article.id, "x");
        assert_eq!(article.title, "T");
        assert_eq!(article.origin, "S");
        assert_eq!(article.keywords, vec!["a", "b"]);
        assert_eq!(
            split_paragraphs(&article.body),
            vec!["First para.", "Second para."]
        );
    }

    /// 真题素材必须真的带出处：否则以后自己都说不清文章是哪来的。
    #[test]
    fn real_exam_articles_carry_their_source() {
        let exam: Vec<&ArticleSource> = builtin_articles()
            .iter()
            .filter(|a| a.id.starts_with("cet4-") || a.id.starts_with("cet6-"))
            .collect();
        assert!(
            exam.len() >= 18,
            "真题选文不应少于 18 篇（实际 {}）",
            exam.len()
        );
        for article in &exam {
            assert!(
                article.origin.contains("真题"),
                "{} 的出处要写明是哪套真题（实际：{}）",
                article.id,
                article.origin
            );
            // 注意：**不要求 keywords 非空**。批量导入的批次（脚本抓取）暂时没有核心词标注，
            // 阅读页对无核心词的文章只是不高亮，功能不受影响；
            // 而非空的 keywords 必须在正文/标题里出现 —— 那条由 `builtin_articles_are_well_formed` 保证。
        }
    }
}
