#!/usr/bin/env node
/**
 * 批量导入四六级「长篇阅读（Part III Section B）」真题选文到 App 内置素材。
 *
 * 数据源：懒笔记（english-exam.lazynote.cn）的「试卷排版」页——它按真题原卷版式
 * 还原卷面，只有英文原文与少量中文生词注解，是最干净的抓取目标。
 *
 * 用法：
 *   node scripts/import-cet-articles.mjs --exam cet4 --year 2022
 *   node scripts/import-cet-articles.mjs --exam all --year 2022 --limit 12
 *   node scripts/import-cet-articles.mjs --exam cet6 --dry-run      # 只看会导入哪些
 *
 * 参数：
 *   --exam    cet4 | cet6 | all（默认 all）
 *   --year    只导入该年份（如 2022）；不传则全部
 *   --limit   最多导入 N 篇（默认不限）
 *   --delay   每篇之间的间隔毫秒（默认 400，别把人家站点打疼）
 *   --dry-run 只打印将要导入的清单，不写文件
 *
 * 产物：`src-tauri/assets/articles/<exam>-<YYYY-MM-SET>.md`
 *   front-matter 含 id / title / source / keywords（批量导入的批次 keywords 留空，
 *   阅读页对无核心词的文章不显示高亮提示，其余功能不受影响）。
 *
 * 幂等：已存在同一套真题的 md（按 slug 匹配文件名）会跳过，重复跑不会产生重复文章。
 *
 * 版权：这些是四六级真题阅读选文（本身改编自外刊），仅供个人背单词使用；
 * 出处逐篇写入 front-matter 的 source 字段。
 */
import { readdir, writeFile } from "node:fs/promises";
import path from "node:path";

const SITE = "https://english-exam.lazynote.cn";
const ROOT = path.resolve(import.meta.dirname, "..");
const OUT_DIR = path.join(ROOT, "src-tauri", "assets", "articles");

// --- 命令行参数 -------------------------------------------------------------

const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  const value = i >= 0 ? args[i + 1] : undefined;
  return value && !value.startsWith("--") ? value : fallback;
};
const has = (name) => args.includes(`--${name}`);

const examArg = opt("exam", "all");
const year = opt("year", null);
const limit = Number(opt("limit", "0")) || 0;
const delayMs = Number(opt("delay", "400")) || 400;
const dryRun = has("dry-run");

const exams = examArg === "all" ? ["cet4", "cet6"] : [examArg];
for (const e of exams) {
  if (e !== "cet4" && e !== "cet6") {
    console.error(`未知考试类型：${e}（只支持 cet4 / cet6 / all）`);
    process.exit(1);
  }
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// --- 文本清理 ---------------------------------------------------------------

/** 去标签 + 解 HTML 实体 + 折叠空白 */
function toPlainText(html) {
  return html
    .replace(/<[^>]+>/g, "")
    .replace(/&quot;/g, '"')
    .replace(/&#0?39;/g, "'")
    .replace(/&#x27;/g, "'")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&nbsp;/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

/**
 * 去掉网站给生词加的中文注解：`tacks(大头钉)` → `tacks`、`climate-resilient(具有气候韧性的)`。
 * 只删「括号里含中文」的，所以 `(Apple Inc.)`、`(2020)` 这类正常括号会保留。
 */
function stripChineseGloss(text) {
  return text.replace(/[（(][^()（）]*[\u4e00-\u9fff][^()（）]*[)）]/g, "");
}

/** 卷面上的 markdown 转义与斜体标记对阅读页是噪音（阅读页不渲染 markdown） */
function stripMarkdown(text) {
  return text.replace(/\\\*/g, "").replace(/\*/g, "");
}

function cleanText(html) {
  return toPlainText(stripMarkdown(stripChineseGloss(html)));
}

// --- 抓取与解析 -------------------------------------------------------------

async function fetchText(url) {
  const res = await fetch(url, { signal: AbortSignal.timeout(30000) });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.text();
}

/** 索引页 → 该考试下所有套次的 slug（如 `2022-06-1`） */
async function listPaperSlugs(exam) {
  const html = await fetchText(`${SITE}/${exam}/sections/long-reading/`);
  const re = new RegExp(`href="/${exam}/paper/(\\d{4}-\\d{2}-\\d+)/part3?-section-b/"`, "g");
  const slugs = new Set();
  for (const m of html.matchAll(re)) slugs.add(m[1]);
  return [...slugs].sort();
}

/**
 * 「试卷排版」页 → { title, paragraphs, origin }
 *
 * 只取 `id="paper"`（卷面）到 `id="sources"`（出处）之间的那段 HTML：
 * 页面顶部还有一份目录，目录里同样出现段落文字，不切范围会把目录也当成正文。
 */
function parsePaper(html) {
  const start = html.indexOf('id="paper"');
  const end = html.indexOf('id="sources"');
  if (start < 0) throw new Error("找不到卷面容器（id=paper）");
  const surface = html.slice(start, end > start ? end : start + 80000);

  // 标题：Directions 之后那个加粗居中的 span
  const titleMatch = surface.match(
    /justify-content:center;font-weight:bolder;[^>]*>\s*<span>([\s\S]*?)<\/span>/,
  );
  const title = titleMatch ? cleanText(titleMatch[1]) : "";

  // 段落：`data-pdh-letter` 只出现在卷面段落上（目录用的是另一套 class）
  const paragraphs = [...surface.matchAll(/data-pdh-letter="[A-Z]"[^>]*>([\s\S]*?)<\/div>/g)]
    .map((m) => cleanText(m[1]))
    .filter((p) => p.length > 0);

  // 出处：`<p class="pl-src__line">选文出处：…</p>`
  let origin = "";
  const originMatch = html.match(/<p class="pl-src__line"[^>]*>([\s\S]*?)<\/p>/);
  if (originMatch) {
    origin = cleanText(originMatch[1]).replace(/^选文出处[:：]?\s*/, "").trim();
  }

  return { title, paragraphs, origin };
}

function buildMarkdown({ exam, slug, title, origin, paragraphs }) {
  const [y, m, set] = slug.split("-");
  const examName = exam === "cet4" ? "英语四级" : "英语六级";
  // 一律以「年份套次」开头，括号里才是原刊物出处（或说明该页没标注）。
  // 之前写成「有 origin 就不加 label」，结果 43 篇无出处的文章在 App 里看不出是哪套卷。
  const label = `${y} 年 ${Number(m)} 月${examName}真题 · 第 ${set} 套 Part III Section B`;
  const fullSource = `${label}（${origin || "该页未标注原文出处"}）`;

  return `---
id: ${exam}-${slug}
title: ${title}
source: ${fullSource}
keywords:
---

${paragraphs.join("\n\n")}
`;
}

/**
 * `--refresh`：重新抓取**脚本生成**的素材并覆盖写回。
 *
 * 只处理文件名形如 `cet6-2022-06-1.md` 的（手写命名的 `cet4-2024-06-2-long-term-goals.md`
 * 是人工整理的，keywords 与正文都不应被覆写），也不会新增文件。
 */
async function refreshExisting(existingFiles, exams) {
  const generated = existingFiles.filter((f) => /^cet[46]-\d{4}-\d{2}-\d+\.md$/.test(f));
  console.log(`--refresh：检查 ${generated.length} 个脚本生成的素材\n`);

  let updated = 0;
  let failed = 0;
  for (const [index, file] of generated.entries()) {
    const [exam, ...slugParts] = file.replace(/\.md$/, "").split("-");
    const slug = slugParts.join("-");
    if (!exams.includes(exam)) continue;
    try {
      const html = await fetchText(`${SITE}/${exam}/paper/${slug}/part3-section-b/`);
      const { title, paragraphs, origin } = parsePaper(html);
      if (!title || paragraphs.length < 3) throw new Error("解析结果异常");
      await writeFile(
        path.join(OUT_DIR, file),
        buildMarkdown({ exam, slug, title, origin, paragraphs }),
        "utf8",
      );
      updated += 1;
    } catch (err) {
      failed += 1;
      console.error(`! ${file} 刷新失败：${err.message}`);
    }
    if (index < generated.length - 1) await sleep(delayMs);
  }
  console.log(`\n完成：刷新 ${updated} 个${failed > 0 ? `，失败 ${failed} 个` : ""}`);
}

// --- 主流程 -----------------------------------------------------------------

async function main() {
  const existingFiles = (await readdir(OUT_DIR).catch(() => [])).filter((f) =>
    f.endsWith(".md"),
  );

  // --refresh：只重写脚本生成的素材（不新增、不碰手写命名的那些）
  if (has("refresh")) {
    await refreshExisting(existingFiles, exams);
    return;
  }
  // 判重必须带上**考试前缀**：四六级会有相同的年月套次（如都是 `2023-12-1`），
  // 只用 slug 匹配文件名的话，先导入的 `cet4-2023-12-1-…` 会把
  // `cet6-2023-12-1` 误判成「已导入」而漏掉（实测漏了 10 套）。
  const existing = existingFiles.map((f) => f.replace(/\.md$/, ""));
  const alreadyImported = (exam, slug) =>
    existing.some((name) => name.startsWith(`${exam}-`) && name.includes(slug));

  const targets = [];
  for (const exam of exams) {
    const slugs = await listPaperSlugs(exam);
    for (const slug of slugs) {
      if (year && !slug.startsWith(`${year}-`)) continue;
      if (alreadyImported(exam, slug)) continue;
      targets.push({ exam, slug });
    }
    console.log(`${exam}：索引 ${slugs.length} 套，本次待导入 ${targets.filter((t) => t.exam === exam).length} 套`);
  }

  const picked = limit > 0 ? targets.slice(0, limit) : targets;
  if (picked.length === 0) {
    console.log("没有需要导入的套次（都已存在或年份不匹配）。");
    return;
  }
  console.log(`\n共 ${picked.length} 套：${picked.map((p) => `${p.exam}/${p.slug}`).join(", ")}\n`);

  let ok = 0;
  let failed = 0;

  for (const [index, { exam, slug }] of picked.entries()) {
    const url = `${SITE}/${exam}/paper/${slug}/part3-section-b/`;
    try {
      const html = await fetchText(url);
      const { title, paragraphs, origin } = parsePaper(html);

      if (!title) throw new Error("解析不到标题");
      if (paragraphs.length < 3) throw new Error(`段落数异常（${paragraphs.length}）`);

      const markdown = buildMarkdown({ exam, slug, title, origin, paragraphs });
      const file = `${exam}-${slug}.md`;
      const words = markdown.split(/\s+/).filter(Boolean).length;

      if (dryRun) {
        console.log(`[dry-run] ${file}｜${paragraphs.length} 段｜~${words} 词｜${title}`);
      } else {
        await writeFile(path.join(OUT_DIR, file), markdown, "utf8");
        console.log(`+ ${file}｜${paragraphs.length} 段｜${title}`);
      }
      ok += 1;
    } catch (err) {
      failed += 1;
      console.error(`! ${exam}/${slug} 导入失败：${err.message}`);
    }

    if (index < picked.length - 1) await sleep(delayMs);
  }

  console.log(
    `\n完成：成功 ${ok} 套${dryRun ? "（dry-run，未写文件）" : `，写入 ${OUT_DIR}`}${failed > 0 ? `，失败 ${failed} 套` : ""}`,
  );
  console.log("提示：新增素材后重跑 `cargo test --lib`，会自动校验素材格式与出处标注。");
}

await main();
