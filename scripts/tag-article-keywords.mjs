#!/usr/bin/env node
/**
 * 给内置阅读素材自动标注「四六级核心词」（front-matter 的 keywords）。
 *
 * 为什么要脚本：手写素材的核心词是人工挑的（质量高），但批量导入的 100+ 篇
 * 不可能逐篇人工标注 —— 那就用**大纲词表**来自动挑：
 * 从正文里找「属于 CET4/CET6 大纲词、长度 ≥ 5、出现次数多」的词，取前 N 个。
 *
 * 词表来源（开源，缓存到 `scripts/.cache/cet-words.txt`，不入库）：
 *   mahavivo/english-wordlists 的 CET4_edited.txt + CET6_edited.txt
 * 用 curl 下载而不是 node fetch：node 的 fetch 不读 HTTP_PROXY，
 * 国内直连 GitHub raw 经常失败，curl 会走系统代理。
 *
 * 用法：
 *   node scripts/tag-article-keywords.mjs                # 只补 keywords 为空的
 *   node scripts/tag-article-keywords.mjs --force        # 全部重算（会覆盖人工标注）
 *   node scripts/tag-article-keywords.mjs --limit 15 --dry-run
 */
import { readdir, readFile, writeFile, mkdir } from "node:fs/promises";
import { existsSync } from "node:fs";
import { execFileSync } from "node:child_process";
import path from "node:path";

const ROOT = path.resolve(import.meta.dirname, "..");
const ARTICLES = path.join(ROOT, "src-tauri", "assets", "articles");
const CACHE_DIR = path.join(ROOT, "scripts", ".cache");
const CACHE = path.join(CACHE_DIR, "cet-words.txt");

const CET4_URL =
  "https://raw.githubusercontent.com/mahavivo/english-wordlists/master/CET4_edited.txt";
const CET6_URL =
  "https://raw.githubusercontent.com/mahavivo/english-wordlists/master/CET6_edited.txt";

const args = process.argv.slice(2);
const has = (name) => args.includes(`--${name}`);
const opt = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  const value = i >= 0 ? args[i + 1] : undefined;
  return value && !value.startsWith("--") ? value : fallback;
};

const LIMIT = Number(opt("limit", "12")) || 12;
const FORCE = has("force");
const dryRun = has("dry-run");

/** 用 curl 下载（见文件头注释：node fetch 不走系统代理） */
function download(url) {
  return execFileSync("curl", ["-sL", "--max-time", "60", url], {
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
  });
}

/** 词表每行形如 `abandon [əˈbændən] vt.放弃`；标题行与字母分组行（`A`）都不匹配 */
function parseWordList(text) {
  const words = new Set();
  for (const line of text.split(/\r?\n/)) {
    const m = line.match(/^([a-z][a-z'-]+)\b/);
    if (m) words.add(m[1]);
  }
  return words;
}

async function loadDictionary() {
  if (existsSync(CACHE)) {
    const text = await readFile(CACHE, "utf8");
    return new Set(text.split("\n").map((w) => w.trim()).filter(Boolean));
  }
  console.log("首次运行：下载 CET4/CET6 大纲词表…");
  const words = new Set([...parseWordList(download(CET4_URL)), ...parseWordList(download(CET6_URL))]);
  await mkdir(CACHE_DIR, { recursive: true });
  await writeFile(CACHE, [...words].sort().join("\n"), "utf8");
  console.log(`已缓存 ${words.size} 个词 → ${CACHE}`);
  return words;
}

/**
 * 停用词：通用英语里的高频功能词与超高频基础词。
 *
 * 不排掉它们的话，「出现次数最多的词」永远是 the/their/about 这类，
 * 而它们对「这个人在背四六级词汇」毫无价值（大纲词表本身是含基础词的）。
 */
const STOP_WORDS = new Set(
  `about above after again against almost alone along already also although always among another anymore
   anyone anything because become becomes been before began begin behind being below beside better between both
   bring brought came cannot come could
   did does doing done down during
   each either else enough even ever every everything except
   fact family few find first found from further
   gave give given goes going gone good great
   hard have having here herself himself however
   instead into itself
   keep kind know
   large last late later least leave left less life like likely little long look
   made make making many may maybe mean means might mind more most much must myself
   near need never new next none nothing now
   often once only onto other others ourselves over own
   part people perhaps place please point put
   quite rather really right round
   said same save saw saying says school seem seemed seen several should show since some something sometimes
   still such sure
   take taken takes talk than that their them themselves then there therefore these they thing things think this
   those though through thus time today together too toward turn
   under until upon use used using usually
   value very
   want was water way week well went were what whatever when where whereas whether which while whole whom
   whose why will with within without word work world would
   year yesterday yet young your yourself`.split(/\s+/).filter(Boolean),
);

/**
 * 挑核心词：正文（含标题）里属于大纲词、长度 ≥ 6、不在停用词表、出现次数最多的前 N 个。
 *
 * 门槛定在 6 字母是实验出来的：5 字母会漏进 their/there/about/still 这类词，
 * 6 字母起剩下的基本是真正值得记的实词（abstract、barrier、significant…）。
 * 次数优先是因为文章反复用到的词通常就是它的主题词。
 */
function pickKeywords(title, body, dict, limit) {
  const counts = new Map();
  for (const m of `${title} ${body}`.matchAll(/[A-Za-z][A-Za-z'’-]{4,}/g)) {
    const word = m[0].toLowerCase().replace(/['’-]+$/, "");
    if (word.length < 6 || STOP_WORDS.has(word) || !dict.has(word)) continue;
    counts.set(word, (counts.get(word) ?? 0) + 1);
  }
  return [...counts.entries()]
    .sort((a, b) => b[1] - a[1] || b[0].length - a[0].length || a[0].localeCompare(b[0]))
    .slice(0, limit)
    .map(([word]) => word);
}

function splitArticle(raw) {
  const text = raw.replace(/^\uFEFF/, "");
  const m = text.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/);
  if (!m) throw new Error("缺少 front-matter");
  return { head: m[1], body: m[2] };
}

function withKeywords(head, keywords) {
  const line = `keywords: ${keywords.join(", ")}`;
  return /^keywords:/m.test(head) ? head.replace(/^keywords:.*$/m, line) : `${head}\n${line}`;
}

async function main() {
  const dict = await loadDictionary();
  const files = (await readdir(ARTICLES)).filter((f) => f.endsWith(".md")).sort();

  let tagged = 0;
  let skipped = 0;
  let failed = 0;

  for (const file of files) {
    const full = path.join(ARTICLES, file);
    try {
      const raw = await readFile(full, "utf8");
      const { head, body } = splitArticle(raw);
      const current = (head.match(/^keywords:\s*(.*)$/m)?.[1] ?? "").trim();

      // 默认只补空的：人工标注过的（手写那 21 篇）保留原样
      if (current && !FORCE) {
        skipped += 1;
        continue;
      }

      const title = head.match(/^title:\s*(.*)$/m)?.[1]?.trim() ?? "";
      const keywords = pickKeywords(title, body, dict, LIMIT);
      if (keywords.length === 0) {
        failed += 1;
        console.error(`! ${file}：正文里没找到大纲词`);
        continue;
      }

      if (dryRun) {
        console.log(`[dry-run] ${file} → ${keywords.slice(0, 6).join(", ")}…（${keywords.length} 个）`);
      } else {
        const next = `---\n${withKeywords(head, keywords)}\n---\n${body}`;
        await writeFile(full, next, "utf8");
      }
      tagged += 1;
    } catch (err) {
      failed += 1;
      console.error(`! ${file} 处理失败：${err.message}`);
    }
  }

  console.log(
    `\n完成：${dryRun ? "将标注" : "已标注"} ${tagged} 篇，跳过（已有 keywords）${skipped} 篇${failed > 0 ? `，失败 ${failed} 篇` : ""}`,
  );
  console.log("提示：改完素材跑 `cargo test --lib`，会校验 keywords 确实出现在正文/标题里。");
}

await main();
