import "./style.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open, save } from "@tauri-apps/plugin-dialog";

type Mode = "pack" | "unpack" | "verify" | "migrate" | "recover";
type Theme = "deep" | "day" | "night";
type OperationResult = { filename: string; originalSize: number; output: string };
type Progress = { processed: number; total: number; percent: number };
type Appearance = { theme: Theme; sound: boolean; motion: boolean };
const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const modeButtons = Array.from(document.querySelectorAll<HTMLButtonElement>("[data-mode]"));
const themeButtons = Array.from(document.querySelectorAll<HTMLButtonElement>("[data-theme-choice]"));
const password = $<HTMLInputElement>("password"), newPassword = $<HTMLInputElement>("new-password"), confirmPassword = $<HTMLInputElement>("confirm-password");
const execute = $<HTMLButtonElement>("execute"), validation = $("validation-message"), result = $("result");
const appearancePanel = $("appearance-panel"), appearanceToggle = $<HTMLButtonElement>("appearance-toggle");
const storageKey = "guanhai-bafen-appearance-v1";
const modeCopy: Record<Mode, { heading: string; action: string; password: string; accept: string; title: string; description: string }> = {
  pack: { heading: "选择要封装的文件或文件夹", action: "开始封装", password: "封装口令", accept: "文件或文件夹，可多选", title: "新格式 BFQ", description: "文件名和内容都会受到保护。可批量处理、压缩文件夹，不记录口令。" },
  unpack: { heading: "选择 BFQ 加密包", action: "验证并拆包", password: "拆包口令", accept: "BFQ 加密包，可多选", title: "完整验证后输出", description: "核对口令与文件完整性后才输出；文件夹包会自动恢复原目录。" },
  verify: { heading: "选择要校验的 BFQ 加密包", action: "开始校验", password: "校验口令", accept: "BFQ 加密包，可多选", title: "只校验，不生成文件", description: "完整读取并检查加密包，口令错误或内容损坏会明确提示。" },
  migrate: { heading: "选择旧 EN3 文件", action: "迁移到 BFQ", password: "旧 EN3 口令", accept: "旧 EN3 文件，可多选", title: "直接迁移旧文件", description: "从旧包直接转换为新包，不生成可读的中间文件。" },
  recover: { heading: "选择旧 EN3 文件", action: "恢复旧文件", password: "旧 EN3 口令", accept: "旧 EN3 文件，可多选", title: "旧格式只读恢复", description: "用于救回历史文件。建议恢复后重新封装为 BFQ。" },
};
let mode: Mode = "pack", selectedInputs: string[] = [], selectedOutput = "", running = false;
let appearance: Appearance = { theme: "deep", sound: true, motion: !window.matchMedia("(prefers-reduced-motion: reduce)").matches };
function loadAppearance() { try { const saved = JSON.parse(localStorage.getItem(storageKey) ?? "null") as Partial<Appearance> | null; if (saved) { if (["deep", "day", "night"].includes(saved.theme ?? "")) appearance.theme = saved.theme as Theme; if (typeof saved.sound === "boolean") appearance.sound = saved.sound; if (typeof saved.motion === "boolean") appearance.motion = saved.motion; } } catch { /* Preferences are optional. */ } }
function applyAppearance() {
  document.documentElement.dataset.theme = appearance.theme; document.documentElement.dataset.motion = appearance.motion ? "on" : "off";
  document.querySelector('meta[name="theme-color"]')?.setAttribute("content", { deep: "#2e596c", day: "#1798e8", night: "#000000" }[appearance.theme]);
  for (const button of themeButtons) { const selected = button.dataset.themeChoice === appearance.theme; button.classList.toggle("selected", selected); button.setAttribute("aria-checked", String(selected)); }
  for (const [id, enabled] of [["sound-toggle", appearance.sound], ["motion-toggle", appearance.motion]] as const) $(id).setAttribute("aria-checked", String(enabled));
  $<HTMLButtonElement>("preview-sound").disabled = !appearance.sound;
  try { localStorage.setItem(storageKey, JSON.stringify(appearance)); } catch { /* Keep usable. */ }
}
function setAppearanceOpen(open: boolean) { appearancePanel.hidden = !open; appearanceToggle.setAttribute("aria-expanded", String(open)); }
function basename(path: string) { return path.replaceAll("\\", "/").split("/").filter(Boolean).pop() ?? path; }
function suggestedOutput(path: string) { return path && (mode === "pack" || mode === "migrate") ? path.replace(/\.[^.\\/]+$/, "") + ".bfq" : ""; }
function currentNewPassword() { return mode === "migrate" ? newPassword.value : password.value; }
function passwordStrength() {
  const value = currentNewPassword(); let score = 0;
  if (value.length >= 8) score++; if (value.length >= 14) score++; if (/[a-z]/.test(value) && /[A-Z]/.test(value)) score++; if (/\d/.test(value)) score++; if (/[^A-Za-z0-9]/.test(value)) score++;
  const levels = value ? score <= 2 ? ["偏弱", "#d14424"] : score <= 4 ? ["可用", "#b37a00"] : ["较强", "#29845b"] : ["等待输入", ""];
  $("strength-label").textContent = levels[0]; $("strength-label").style.color = levels[1]; $("strength-fill").style.width = value ? `${Math.max(15, score * 20)}%` : "0"; $("strength-fill").style.background = levels[1];
}
function validate(showError = false) {
  let message = "";
  if (!selectedInputs.length) message = "请选择输入文件或文件夹";
  else if (!password.value) message = mode === "migrate" ? "请输入旧 EN3 口令" : "请输入口令";
  else if (mode === "migrate" && !newPassword.value) message = "请输入新 BFQ 口令";
  else if ((mode === "pack" || mode === "migrate") && !confirmPassword.value) message = "请再次输入口令";
  else if ((mode === "pack" || mode === "migrate") && currentNewPassword() !== confirmPassword.value) message = "两次输入的口令不一致";
  execute.disabled = running || Boolean(message);
  if (!running) { validation.textContent = message || `准备就绪 · ${selectedInputs.length} 项`; validation.classList.toggle("error", showError && Boolean(message)); }
  passwordStrength(); return !message;
}
function updateMode(next: Mode) {
  if (running) return; mode = next; selectedInputs = []; selectedOutput = "";
  for (const button of modeButtons) { const active = button.dataset.mode === mode; button.classList.toggle("active", active); button.setAttribute("aria-selected", String(active)); }
  const copy = modeCopy[mode]; $("file-heading").textContent = copy.heading; $("operation-heading").textContent = mode === "migrate" ? "设置新旧口令" : "设置口令并执行";
  $("password-label").textContent = copy.password; execute.textContent = copy.action; $("format-title").textContent = copy.title; $("format-description").textContent = copy.description;
  $("new-password-group").hidden = mode !== "migrate"; $("confirm-group").hidden = !["pack", "migrate"].includes(mode); $("generate-password").hidden = !["pack", "migrate"].includes(mode);
  $("pack-options").hidden = mode !== "pack"; $("strength-meter").hidden = !["pack", "migrate"].includes(mode); $("output-block").hidden = mode === "verify";
  confirmPassword.value = ""; newPassword.value = ""; renderPaths(); validate();
}
function renderPaths() {
  const multiple = selectedInputs.length > 1, first = selectedInputs[0] ?? "";
  $("input-path").textContent = multiple ? selectedInputs.map(basename).join("、") : first || "—"; $("input-path").classList.toggle("empty", !first);
  $("input-state").textContent = first ? `已选 ${selectedInputs.length} 项` : "未选择";
  $("output-path").textContent = multiple ? "各项自动保存在源文件旁" : selectedOutput || (mode === "verify" ? "不生成文件" : ["pack", "migrate"].includes(mode) ? "自动生成 .bfq 文件" : "自动使用包内原文件名");
  $("output-path").classList.toggle("empty", !selectedOutput); $<HTMLButtonElement>("choose-output").disabled = multiple || mode === "verify";
  $("drop-title").textContent = first ? multiple ? `已选择 ${selectedInputs.length} 项` : basename(first) : "把文件或文件夹拖到这里";
  $("drop-subtitle").textContent = first ? "可重新选择或拖入以替换" : `${modeCopy[mode].accept}，逐项安全处理`;
}
function setInputs(paths: string[]) { selectedInputs = [...new Set(paths)]; selectedOutput = suggestedOutput(selectedInputs[0] ?? ""); result.className = "result"; result.innerHTML = '<span class="result-mark">—</span><p>默认不覆盖已有文件，可开启自动改名。</p>'; renderPaths(); validate(); }
async function chooseInput(directory = false) {
  const filters = mode === "pack" ? [] : [{ name: mode === "migrate" || mode === "recover" ? "EN3 旧文件" : "BFQ 加密包", extensions: mode === "migrate" || mode === "recover" ? ["en3"] : ["bfq"] }];
  const paths = await open({ multiple: !directory, directory, filters }) as string | string[] | null; if (typeof paths === "string") setInputs([paths]); else if (paths?.length) setInputs(paths);
}
async function chooseOutput() { if (!selectedInputs.length) return validate(true); const path = await save({ defaultPath: selectedOutput || "recovered.bin" }); if (path) { selectedOutput = path; renderPaths(); } }
function setBusy(value: boolean) {
  running = value; $("progress-area").hidden = !value; for (const button of modeButtons) button.disabled = value;
  for (const field of [password, newPassword, confirmPassword]) field.disabled = value; $<HTMLButtonElement>("cancel").disabled = !value; validate();
  if (value) { validation.textContent = "正在本机逐项处理"; $("progress-fill").style.width = "0%"; $("progress-percent").textContent = "0%"; $("progress-status").textContent = "正在准备"; }
}
function friendlyError(error: unknown) { const message = String(error); if (message.includes("Operation cancelled") || message.includes("操作已取消")) return "操作已取消"; if (message.includes("Output already exists")) return "目标已存在，未覆盖"; if (message.includes("Wrong password") || message.includes("authentication failed")) return "口令不正确，或文件已经损坏"; if (message.includes("Not an enc3") || message.includes("Not a BFQ")) return "文件与当前操作不匹配"; return message; }
async function runOperation() {
  if (!validate(true)) return; setBusy(true); result.className = "result"; result.innerHTML = '<span class="result-mark">···</span><p>逐项处理，失败项不会中断其他项目。</p>';
  const succeeded: string[] = [], failed: string[] = []; let cancelled = false;
  for (let index = 0; index < selectedInputs.length; index++) {
    const input = selectedInputs[index]; $("progress-status").textContent = `第 ${index + 1}/${selectedInputs.length} 项 · ${basename(input)}`;
    try {
      const response = await invoke<OperationResult>("execute_operation", { request: { operation: mode, input, password: password.value, newPassword: mode === "migrate" ? newPassword.value : null, output: selectedInputs.length === 1 ? selectedOutput || null : null, compress: $<HTMLInputElement>("compress").checked, renameExisting: $<HTMLInputElement>("rename-existing").checked } });
      succeeded.push(mode === "verify" ? `${basename(input)}：完整` : response.output);
    } catch (error) { const message = friendlyError(error); failed.push(`${basename(input)}：${message}`); if (message.includes("已取消")) { cancelled = true; break; } }
  }
  result.className = `result ${failed.length ? "error" : "success"}`;
  result.innerHTML = `<span class="result-mark">${failed.length ? "!" : "✓"}</span><p>${cancelled ? "已取消，" : "处理结束，"}成功 ${succeeded.length} 项，失败 ${failed.length} 项。</p>${succeeded.length ? `<ul>${succeeded.map(v => `<li>✓ ${escapeHtml(v)}</li>`).join("")}</ul>` : ""}${failed.length ? `<ul>${failed.map(v => `<li>× ${escapeHtml(v)}</li>`).join("")}</ul>` : ""}`;
  password.value = ""; newPassword.value = ""; confirmPassword.value = ""; setBusy(false); renderPaths();
}
function escapeHtml(value: string) { return value.replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c] ?? c); }
function generatePassword() { const alphabet = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789!@#$%_-", random = new Uint32Array(24); crypto.getRandomValues(random); const value = Array.from(random, n => alphabet[n % alphabet.length]).join(""); if (mode === "migrate") newPassword.value = value; else password.value = value; confirmPassword.value = value; validate(); }
for (const button of modeButtons) button.addEventListener("click", () => updateMode(button.dataset.mode as Mode));
$("choose-input").addEventListener("click", () => void chooseInput()); $("choose-folder").addEventListener("click", () => void chooseInput(true));
$("drop-zone").addEventListener("click", event => { if (!["choose-input", "choose-folder"].includes((event.target as HTMLElement).id) && !running) void chooseInput(); });
$("drop-zone").addEventListener("keydown", event => { if (event.key === "Enter" || event.key === " ") void chooseInput(); });
$("choose-output").addEventListener("click", chooseOutput); execute.addEventListener("click", runOperation); $("generate-password").addEventListener("click", generatePassword);
for (const [button, field] of [["toggle-password", password], ["toggle-new-password", newPassword]] as const) $(button).addEventListener("click", () => { field.type = field.type === "password" ? "text" : "password"; $(button).textContent = field.type === "password" ? "显示" : "隐藏"; });
$("cancel").addEventListener("click", async () => { await invoke("cancel_operation"); $("progress-status").textContent = "正在取消"; $<HTMLButtonElement>("cancel").disabled = true; });
for (const field of [password, newPassword, confirmPassword]) field.addEventListener("input", () => validate());
appearanceToggle.addEventListener("click", () => setAppearanceOpen(appearancePanel.hidden)); $("appearance-close").addEventListener("click", () => setAppearanceOpen(false));
for (const button of themeButtons) button.addEventListener("click", () => { appearance.theme = button.dataset.themeChoice as Theme; applyAppearance(); });
$("sound-toggle").addEventListener("click", () => { appearance.sound = !appearance.sound; applyAppearance(); }); $("motion-toggle").addEventListener("click", () => { appearance.motion = !appearance.motion; applyAppearance(); });
document.addEventListener("pointerdown", event => { const target = event.target as Node; if (!appearancePanel.hidden && !appearancePanel.contains(target) && !appearanceToggle.contains(target)) setAppearanceOpen(false); }); document.addEventListener("keydown", event => { if (event.key === "Escape") setAppearanceOpen(false); });
$("file-column").addEventListener("pointermove", event => { if (!appearance.motion) return; const column = event.currentTarget as HTMLElement, b = column.getBoundingClientRect(); column.style.setProperty("--pointer-x", String(((event.clientX - b.left) / b.width - .5) * 2)); column.style.setProperty("--pointer-y", String(((event.clientY - b.top) / b.height - .5) * 2)); });
void listen<Progress>("operation-progress", event => { const p = event.payload; $("progress-status").textContent = `${formatBytes(p.processed)} / ${formatBytes(p.total)}`; $("progress-percent").textContent = `${p.percent}%`; $("progress-fill").style.width = `${p.percent}%`; $("progress-fill").parentElement?.setAttribute("aria-valuenow", String(p.percent)); });
function formatBytes(bytes: number) { if (bytes < 1024) return `${bytes} B`; if (bytes < 1048576) return `${(bytes / 1024).toFixed(1)} KB`; return `${(bytes / 1048576).toFixed(1)} MB`; }
void getCurrentWindow().onDragDropEvent(event => { const type = event.payload.type; $("drop-zone").classList.toggle("dragging", type === "enter" || type === "over"); if (type === "drop" && !running && event.payload.paths.length) setInputs(event.payload.paths); });
let audioContext: AudioContext | null = null, startupSoundPlayed = false, startupSoundPending = false;
async function playChime() { const AudioConstructor = window.AudioContext ?? (window as typeof window & { webkitAudioContext?: typeof AudioContext }).webkitAudioContext; if (!AudioConstructor) throw new Error("Audio unavailable"); audioContext ??= new AudioConstructor(); if (audioContext.state === "suspended") await audioContext.resume(); if (audioContext.state !== "running") throw new Error("Audio locked"); const now = audioContext.currentTime + .015, master = audioContext.createGain(); master.gain.setValueAtTime(.16, now); master.connect(audioContext.destination); [392, 523.25, 659.25].forEach((frequency, index) => { const start = now + index * .105, oscillator = audioContext!.createOscillator(), gain = audioContext!.createGain(); oscillator.type = index === 2 ? "sine" : "triangle"; oscillator.frequency.setValueAtTime(frequency, start); gain.gain.setValueAtTime(.0001, start); gain.gain.exponentialRampToValueAtTime(index === 2 ? .24 : .16, start + .025); gain.gain.exponentialRampToValueAtTime(.0001, start + .52); oscillator.connect(gain); gain.connect(master); oscillator.start(start); oscillator.stop(start + .55); }); }
async function tryStartupChime(preview = false) { if (!appearance.sound || (startupSoundPlayed && !preview)) return; try { await playChime(); if (!preview) startupSoundPlayed = true; startupSoundPending = false; } catch { if (!preview) startupSoundPending = true; } }
$("preview-sound").addEventListener("click", () => void tryStartupChime(true)); window.addEventListener("pointerdown", () => { if (startupSoundPending && !splashDismissed) void tryStartupChime(); }, { passive: true });
loadAppearance(); applyAppearance(); updateMode("pack");
const splash = $("splash"); let splashDismissed = false;
function dismissSplash() { if (splashDismissed) return; splashDismissed = true; splash.classList.add("leaving"); const finish = () => { splash.hidden = true; }; splash.addEventListener("animationend", finish, { once: true }); window.setTimeout(finish, 550); }
$("skip-splash").addEventListener("click", dismissSplash); window.setTimeout(() => void tryStartupChime(), appearance.motion ? 760 : 100); window.setTimeout(dismissSplash, appearance.motion ? 2500 : 120);
