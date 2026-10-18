#!/usr/bin/env node
/**
 * 把 APK 与版本元数据发布成 **GitHub Release**，供手机端「公网更新通道」拉取。
 *
 * 为什么需要它：手机端要「不依赖电脑更新」，就必须有个公网源。GitHub Releases 是现成的——
 * 客户端读的是 `releases/latest/download/pointra.json` 与 `.../pointra.apk`，
 * 所以每次发版只要把这两个文件传成 release 附件即可，客户端不用改。
 *
 * 用法：
 *   node scripts/publish-github-release.mjs            # 用当前版本（tauri.conf.json）
 *   node scripts/publish-github-release.mjs --dry-run   # 只看要发什么
 *
 * Token 从哪儿来（**都不要提交进仓库**）：
 *   1. 环境变量 `GITHUB_TOKEN`；或
 *   2. 仓库外的私人文件 `~/.pointra-github-token`（推荐，脚本不回显内容）
 * 需要的权限：Fine-grained token → 该仓库的 Contents: Read and write（+ Metadata: Read）。
 *
 * 前置：先跑 `scripts/publish-update.ps1`（它会把 APK 与带 notes 的元数据放进
 * `%APPDATA%\com.syjun.pointra\update\`），本脚本直接复用那两个文件，保证
 * **公网源与电脑端源内容完全一致**。
 *
 * 注：用 curl 而不是 node 的 fetch —— fetch 不读系统代理，国内直连 api.github.com 常失败。
 */
import { readFile } from "node:fs/promises";
import {
  existsSync,
  readFileSync,
  unlinkSync,
  writeFileSync,
  mkdirSync,
  copyFileSync,
} from "node:fs";
import { execFileSync } from "node:child_process";
import os from "node:os";
import path from "node:path";

const REPO = "houakun/Pointra-Mobile";
const ROOT = path.resolve(import.meta.dirname, "..");
const UPDATE_DIR = path.join(process.env.APPDATA ?? "", "com.syjun.pointra", "update");
const TOKEN_FILE = path.join(os.homedir(), ".pointra-github-token");

const args = process.argv.slice(2);
const dryRun = args.includes("--dry-run");

function loadToken() {
  const fromEnv = process.env.GITHUB_TOKEN?.trim();
  if (fromEnv) return { token: fromEnv, from: "环境变量 GITHUB_TOKEN" };
  if (existsSync(TOKEN_FILE)) {
    const token = readFileSync(TOKEN_FILE, "utf8").trim();
    if (token) return { token, from: TOKEN_FILE };
  }
  return null;
}

/** 走 curl（会走系统代理），返回 { status, body } */
function request(method, url, { token, json, binary, contentType }) {
  const argv = [
    "-sS",
    "-X",
    method,
    "-H",
    `Authorization: Bearer ${token}`,
    "-H",
    "Accept: application/vnd.github+json",
    "-H",
    "X-GitHub-Api-Version: 2022-11-28",
    "-w",
    "\n%{http_code}",
  ];

  const tempFiles = [];
  if (json !== undefined) {
    // 【易错点】请求体必须走临时文件，不能当命令行参数传：
    // Windows 下 execFileSync 会把含中文的参数按 GBK 交给 curl，
    // GitHub 收到非法 UTF-8 就报 400 “Problems parsing JSON”（踩过）。
    // 另外要用 --data-binary（而不是 -d）才能保留更新说明里的换行。
    const tmp = path.join(os.tmpdir(), `pointra-gh-${process.pid}-${Date.now()}.json`);
    writeFileSync(tmp, JSON.stringify(json), "utf8");
    tempFiles.push(tmp);
    argv.push("-H", "Content-Type: application/json", "--data-binary", `@${tmp}`);
  }
  if (binary) {
    argv.push("-H", `Content-Type: ${contentType ?? "application/octet-stream"}`, "--data-binary", `@${binary}`);
  }
  argv.push(url);

  let out;
  try {
    out = execFileSync("curl", argv, {
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
    });
  } finally {
    // 不管成败都要清理这个临时请求体（里面含更新说明，不该留在磁盘上）
    for (const file of tempFiles) {
      try {
        unlinkSync(file);
      } catch {
        /* 清理失败不影响主流程 */
      }
    }
  }
  const cut = out.lastIndexOf("\n");
  const status = Number(out.slice(cut + 1));
  const text = out.slice(0, cut);
  let body = null;
  try {
    body = text ? JSON.parse(text) : null;
  } catch {
    body = { raw: text.slice(0, 400) };
  }
  return { status, body };
}

/** 读 JSON 并给出人话错误：配置文件损坏时不该甩一坨栈出来 */
async function readJsonFile(file, what) {
  let text;
  try {
    text = await readFile(file, "utf8");
  } catch (err) {
    console.error(`读不到${what}：${file}（${err.code ?? err.message}）`);
    process.exit(1);
  }
  try {
    return JSON.parse(text);
  } catch (err) {
    console.error(`${what}不是合法 JSON：${file}\n  ${err.message}`);
    process.exit(1);
  }
}

async function main() {
  const auth = loadToken();
  if (!auth && !dryRun) {
    console.error("没找到 token。请二选一：");
    console.error(`  1) 把 token 写进 ${TOKEN_FILE}（推荐，文件别进仓库）`);
    console.error("  2) 设置环境变量 GITHUB_TOKEN（需重启 pi 进程）");
    process.exit(1);
  }
  console.log(
    auth
      ? `已读取 token（来自 ${auth.from}，不显示内容）`
      : "（--dry-run：没 token 也继续，只看要发什么）",
  );

  const tauriConf = await readJsonFile(
    path.join(ROOT, "src-tauri", "tauri.conf.json"),
    "tauri.conf.json",
  );
  const version = tauriConf.version;
  const tag = `v${version}`;

  const apkPath = path.join(UPDATE_DIR, "pointra.apk");
  const metaPath = path.join(UPDATE_DIR, "pointra.json");
  for (const p of [apkPath, metaPath]) {
    if (!existsSync(p)) {
      console.error(`找不到 ${p}`);
      console.error("请先跑：powershell -ExecutionPolicy Bypass -File scripts\\publish-update.ps1");
      process.exit(1);
    }
  }
  const meta = await readJsonFile(metaPath, "版本元数据 pointra.json");
  const firstNote = (meta.notes ?? "").split("\n")[0];

  console.log(`准备发布 release ${tag}（versionCode=${meta.version_code}）`);
  console.log(`  附件 1: pointra.apk`);
  console.log(`  附件 2: pointra.json（说明：${firstNote}）`);
  if (dryRun) {
    console.log("\n--dry-run：未调用 GitHub API。");
    return;
  }

  // 1) release 已存在就复用（幂等：重复跑不会报“tag already exists”）
  let release = request("GET", `https://api.github.com/repos/${REPO}/releases/tags/${tag}`, {
    token: auth.token,
  });
  if (release.status === 404) {
    release = request("POST", `https://api.github.com/repos/${REPO}/releases`, {
      token: auth.token,
      json: {
        tag_name: tag,
        name: tag,
        body: meta.notes ?? "",
        draft: false,
        prerelease: false,
      },
    });
  }
  if (release.status >= 300 || !release.body?.id) {
    console.error(`创建/读取 release 失败（HTTP ${release.status}）`);
    console.error(JSON.stringify(release.body, null, 2));
    process.exit(1);
  }
  const releaseId = release.body.id;
  const uploadBase = String(release.body.upload_url).replace(/\{.*$/, "");
  console.log(`release 就绪：${release.body.html_url ?? tag}`);

  // 2) 同名附件先删后传，保证内容与本次构建一致
  const assets = request(
    "GET",
    `https://api.github.com/repos/${REPO}/releases/${releaseId}/assets`,
    { token: auth.token },
  );
  const existing = Array.isArray(assets.body) ? assets.body : [];

  for (const [file, name] of [
    [apkPath, "pointra.apk"],
    [metaPath, "pointra.json"],
  ]) {
    const old = existing.find((a) => a.name === name);
    if (old) {
      const del = request(
        "DELETE",
        `https://api.github.com/repos/${REPO}/releases/assets/${old.id}`,
        { token: auth.token },
      );
      console.log(`  覆盖旧附件 ${name}（HTTP ${del.status}）`);
    }
    const up = request("POST", `${uploadBase}?name=${encodeURIComponent(name)}`, {
      token: auth.token,
      binary: file,
    });
    if (up.status >= 300) {
      console.error(`上传 ${name} 失败（HTTP ${up.status}）`);
      console.error(JSON.stringify(up.body, null, 2));
      process.exit(1);
    }
    console.log(`  已上传 ${name}`);
  }

  console.log("");
  console.log("发布完成。手机端「检查更新」现在会优先走公网：");
  console.log(`  https://github.com/${REPO}/releases/latest/download/pointra.json`);
  console.log("（电脑没开也能更新；公网不通时会自动回落电脑端）");

  // 4) 同步一份到按版本命名的 CDN 分支，给 jsDelivr 用。
  //
  // 为什么另开分支而不是直接用 release 附件：GitHub 的附件要 302 到
  // objects.githubusercontent.com，国内常被阻断（能检查更新但下载卡住）。
  // 为什么分支名带版本号：jsDelivr 对 git ref 的内容有最长 12 小时缓存，
  // 固定分支会让手机在发版后拿到旧包；新 ref 是全新 URL，无缓存问题。
  const cdnBranch = `cdn-v${meta.version}`;
  const cdnWork = path.join(os.homedir(), ".pointra-cdn");
  mkdirSync(cdnWork, { recursive: true });
  copyFileSync(apkPath, path.join(cdnWork, "pointra.apk"));
  copyFileSync(metaPath, path.join(cdnWork, "pointra.json"));

  const git = (...gitArgs) =>
    execFileSync("git", gitArgs, {
      cwd: cdnWork,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
  try {
    git("init", "-q");
    git("checkout", "-q", "-B", cdnBranch);
    git("add", "-A");
    git(
      "-c",
      "user.email=release@local",
      "-c",
      "user.name=release",
      "commit",
      "-q",
      "--allow-empty",
      "-m",
      `cdn: ${meta.version}`,
    );
    try {
      git("remote", "remove", "origin");
    } catch {
      /* 首次没有 remote */
    }
    git("remote", "add", "origin", `https://github.com/${REPO}.git`);

    const pushBranch = (branch, message) => {
      git("checkout", "-q", "-B", branch);
      git("add", "-A");
      git(
        "-c",
        "user.email=release@local",
        "-c",
        "user.name=release",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        message,
      );
      git("push", "-f", "origin", branch);
    };

    pushBranch(cdnBranch, `cdn: ${meta.version}`);
    console.log("");
    console.log(`CDN 分支已推送（${cdnBranch}）——手机端无需 VPN 的下载地址：`);
    console.log(`  https://cdn.jsdelivr.net/gh/${REPO}@${cdnBranch}/pointra.apk`);

    // 5) 再推一个**固定分支** `cdn-latest`，供「检查更新」用。
    //
    // 为什么需要它：检查更新本来只碰 GitHub 的 `releases/latest/download/...`，
    // 那个地址会 302 到 objects.githubusercontent.com —— 国内常被阻断，
    // 于是“检查更新”经常超时（而 jsDelivr 能通，却只在下载时才用）。
    // 固定分支的代价是 jsDelivr 对 ref 有最长 12 小时缓存，所以推完必须 purge。
    const latestBranch = "cdn-latest";
    pushBranch(latestBranch, `cdn latest: ${meta.version}`);
    for (const file of ["pointra.json", "pointra.apk"]) {
      const purgeUrl = `https://purge.jsdelivr.net/gh/${REPO}@${latestBranch}/${file}`;
      try {
        execFileSync("curl", ["-sS", "--max-time", "20", purgeUrl], { encoding: "utf8" });
      } catch (err) {
        console.error(`！刷新 CDN 缓存失败（${file}）：${err.message}`);
      }
    }
    console.log(
      `固定分支已推送并刷新缓存（${latestBranch}）——检查更新走这个源：\n` +
        `  https://cdn.jsdelivr.net/gh/${REPO}@${latestBranch}/pointra.json`,
    );
  } catch (err) {
    console.error(`！CDN 分支推送失败（不影响上面的 release）：${err.message}`);
  }
}

await main();
