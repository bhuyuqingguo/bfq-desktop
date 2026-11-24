import "./style.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open, save } from "@tauri-apps/plugin-dialog";

type Mode = "pack" | "unpack" | "migrate" | "recover";
type OperationResult = { filename: string; originalSize: number; output: string };
type Progress = { processed: number; total: number; percent: number };

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const modeButtons = Array.from(document.querySelectorAll<HTMLButtonElement>("[data-mode]"));
const inputPath = $("input-path");
const outputPath = $("output-path");
const password = $<HTMLInputElement>("password");
const newPassword = $<HTMLInputElement>("new-password");
const confirmPassword = $<HTMLInputElement>("confirm-password");
const execute = $<HTMLButtonElement>("execute");
const validation = $("validation-message");
const progressArea = $("progress-area");
const result = $("result");

const modeCopy: Record<Mode, { heading: string; action: string; password: string; accept: string; title: string; description: string }> = {
  pack: {
    heading: "选择要封装的文件", action: "开始封装", password: "封装口令", accept: "任意文件",
    title: "新格式 BFQ", description: "文件名和内容均加密。每次封装使用新的随机盐，不记录口令。",
  },
  unpack: {
    heading: "选择 BFQ 加密包", action: "认证并拆包", password: "拆包口令", accept: "BFQ 加密包",
    title: "完整认证后输出", description: "逐块验证密文、块序号和终止标记；错误或篡改不会留下输出文件。",
  },
  migrate: {
    heading: "选择旧 EN3 文件", action: "迁移到 BFQ", password: "旧 EN3 口令", accept: "旧 EN3 文件",
    title: "直接流式迁移", description: "旧包解密流直接进入新格式加密流，不生成明文中间文件。",
  },
  recover: {
    heading: "选择旧 EN3 文件", action: "恢复旧文件", password: "旧 EN3 口令", accept: "旧 EN3 文件",
    title: "旧格式只读恢复", description: "用于救回历史文件。旧格式不再生成，建议恢复后重新封装为 BFQ。",
  },
};

let mode: Mode = "pack";
let selectedInput = "";
let selectedOutput = "";
let running = false;

function basename(path: string) {
  return path.replaceAll("\\", "/").split("/").filter(Boolean).pop() ?? path;
}

function suggestedOutput(path: string) {
  if (!path || (mode !== "pack" && mode !== "migrate")) return "";
  return path.replace(/\.[^.\\/]+$/, "") + ".bfq";
}

function currentNewPassword() {
  return mode === "migrate" ? newPassword.value : password.value;
}

function validate(showError = false) {
  let message = "";
  if (!selectedInput) message = "请选择输入文件";
  else if (!password.value) message = mode === "migrate" ? "请输入旧 EN3 口令" : "请输入口令";
  else if (mode === "migrate" && !newPassword.value) message = "请输入新 BFQ 口令";
  else if ((mode === "pack" || mode === "migrate") && !confirmPassword.value) message = "请再次输入口令";
  else if ((mode === "pack" || mode === "migrate") && currentNewPassword() !== confirmPassword.value) message = "两次输入的口令不一致";
  execute.disabled = running || Boolean(message);
  if (!running) {
    validation.textContent = message || "准备就绪";
    validation.classList.toggle("error", showError && Boolean(message));
  }
  return !message;
}

function updateMode(next: Mode) {
  if (running) return;
  mode = next;
  selectedOutput = suggestedOutput(selectedInput);
  for (const button of modeButtons) {
    const active = button.dataset.mode === mode;
    button.classList.toggle("active", active);
    button.setAttribute("aria-selected", String(active));
  }
  const copy = modeCopy[mode];
  $("file-heading").textContent = copy.heading;
  $("operation-heading").textContent = mode === "migrate" ? "设置新旧口令" : "设置口令并执行";
  $("password-label").textContent = copy.password;
  execute.textContent = copy.action;
  $("format-title").textContent = copy.title;
  $("format-description").textContent = copy.description;
  $("new-password-group").hidden = mode !== "migrate";
  $("confirm-group").hidden = mode === "unpack" || mode === "recover";
  $("generate-password").hidden = mode === "unpack" || mode === "recover";
  confirmPassword.value = "";
  newPassword.value = "";
  renderPaths();
  validate();
}

function renderPaths() {
  inputPath.textContent = selectedInput || "—";
  inputPath.classList.toggle("empty", !selectedInput);
  $("input-state").textContent = selectedInput ? basename(selectedInput) : "未选择";
  outputPath.textContent = selectedOutput || (mode === "pack" || mode === "migrate" ? "自动生成 .bfq 文件" : "自动使用包内原文件名");
  outputPath.classList.toggle("empty", !selectedOutput);
  $("drop-title").textContent = selectedInput ? basename(selectedInput) : "把文件拖到这里";
  $("drop-subtitle").textContent = selectedInput ? "已选择，可重新拖入文件替换" : `${modeCopy[mode].accept}，内容将流式处理`;
}

function setInput(path: string) {
  selectedInput = path;
  selectedOutput = suggestedOutput(path);
  result.className = "result";
  result.innerHTML = '<span class="result-mark">—</span><p>输出文件已存在时会停止，不会覆盖原文件。</p>';
  renderPaths();
  validate();
}

async function chooseInput() {
  const filters = mode === "pack" ? [] : [{ name: mode === "unpack" ? "BFQ 加密包" : "EN3 旧文件", extensions: mode === "unpack" ? ["bfq"] : ["en3"] }];
  const path = await open({ multiple: false, directory: false, filters });
  if (typeof path === "string") setInput(path);
}

async function chooseOutput() {
  if (!selectedInput) {
    validate(true);
    return;
  }
  const path = await save({ defaultPath: selectedOutput || "recovered.bin" });
  if (path) {
    selectedOutput = path;
    renderPaths();
  }
}

function setBusy(value: boolean) {
  running = value;
  progressArea.hidden = !value;
  for (const button of modeButtons) button.disabled = value;
  password.disabled = value;
  newPassword.disabled = value;
  confirmPassword.disabled = value;
  $<HTMLButtonElement>("cancel").disabled = !value;
  validate();
  if (value) {
    validation.textContent = "密钥派生和文件处理在后台执行";
    validation.classList.remove("error");
    $("progress-fill").style.width = "0%";
    $("progress-percent").textContent = "0%";
    $("progress-status").textContent = "正在初始化";
  }
}

async function runOperation() {
  if (!validate(true)) return;
  setBusy(true);
  result.className = "result";
  result.innerHTML = '<span class="result-mark">···</span><p>正在安全写入临时文件，完成后才会生成目标文件。</p>';
  try {
    const response = await invoke<OperationResult>("execute_operation", {
      request: {
        operation: mode,
        input: selectedInput,
        password: password.value,
        newPassword: mode === "migrate" ? newPassword.value : null,
        output: selectedOutput || null,
      },
    });
    selectedOutput = response.output;
    result.className = "result success";
    result.innerHTML = `<span class="result-mark">✓</span><p>完成：${escapeHtml(response.output)}</p>`;
    $("progress-status").textContent = "处理完成";
    $("progress-percent").textContent = "100%";
    $("progress-fill").style.width = "100%";
  } catch (error) {
    result.className = "result error";
    result.innerHTML = `<span class="result-mark">×</span><p>${escapeHtml(String(error))}</p>`;
  } finally {
    setBusy(false);
    renderPaths();
  }
}

function escapeHtml(value: string) {
  return value.replace(/[&<>"']/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[character] ?? character);
}

function generatePassword() {
  const alphabet = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789!@#$%_-";
  const random = new Uint32Array(24);
  crypto.getRandomValues(random);
  const value = Array.from(random, (number) => alphabet[number % alphabet.length]).join("");
  if (mode === "migrate") newPassword.value = value;
  else password.value = value;
  confirmPassword.value = value;
  validate();
}

for (const button of modeButtons) button.addEventListener("click", () => updateMode(button.dataset.mode as Mode));
$("choose-input").addEventListener("click", chooseInput);
$("drop-zone").addEventListener("click", (event) => {
  if ((event.target as HTMLElement).id !== "choose-input" && !running) void chooseInput();
});
$("drop-zone").addEventListener("keydown", (event) => { if (event.key === "Enter" || event.key === " ") void chooseInput(); });
$("choose-output").addEventListener("click", chooseOutput);
$("execute").addEventListener("click", runOperation);
$("generate-password").addEventListener("click", generatePassword);
$("toggle-password").addEventListener("click", () => {
  password.type = password.type === "password" ? "text" : "password";
  $("toggle-password").textContent = password.type === "password" ? "显示" : "隐藏";
});
$("toggle-new-password").addEventListener("click", () => {
  newPassword.type = newPassword.type === "password" ? "text" : "password";
  $("toggle-new-password").textContent = newPassword.type === "password" ? "显示" : "隐藏";
});
$("cancel").addEventListener("click", async () => {
  await invoke("cancel_operation");
  $("progress-status").textContent = "正在取消";
  $<HTMLButtonElement>("cancel").disabled = true;
});
for (const field of [password, newPassword, confirmPassword]) field.addEventListener("input", () => validate());

void listen<Progress>("operation-progress", (event) => {
  const progress = event.payload;
  $("progress-status").textContent = `${formatBytes(progress.processed)} / ${formatBytes(progress.total)}`;
  $("progress-percent").textContent = `${progress.percent}%`;
  $("progress-fill").style.width = `${progress.percent}%`;
  $("progress-fill").parentElement?.setAttribute("aria-valuenow", String(progress.percent));
});

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MiB`;
}

void getCurrentWindow().onDragDropEvent((event) => {
  const type = event.payload.type;
  $("drop-zone").classList.toggle("dragging", type === "enter" || type === "over");
  if (type === "drop" && !running && event.payload.paths.length > 0) setInput(event.payload.paths[0]);
});

updateMode("pack");

const splash = $("splash");
let splashDismissed = false;
function dismissSplash() {
  if (splashDismissed) return;
  splashDismissed = true;
  splash.classList.add("leaving");
  const finish = () => { splash.hidden = true; };
  splash.addEventListener("animationend", finish, { once: true });
  window.setTimeout(finish, 550);
}
$("skip-splash").addEventListener("click", dismissSplash);
window.setTimeout(dismissSplash, window.matchMedia("(prefers-reduced-motion: reduce)").matches ? 80 : 2300);
