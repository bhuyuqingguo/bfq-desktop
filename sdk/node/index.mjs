import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";

export class BafenError extends Error {
  constructor(message) { super(message); this.name = "BafenError"; }
}

export class BafenCore {
  constructor({ executable = process.env.GUANHAI_CORE || "guanhai-core" } = {}) {
    this.executable = executable;
  }

  async execute({ operation, input, password, newPassword = null, output = null, compress = false, renameExisting = false }, { onProgress, signal } = {}) {
    if (!["pack", "unpack", "verify", "migrate", "recover"].includes(operation)) throw new BafenError("不支持的操作");
    const temporary = await mkdtemp(join(tmpdir(), "guanhai-bafen-"));
    const cancelFile = join(temporary, "cancel.requested");
    const cancel = () => void writeFile(cancelFile, "cancelled").catch(() => {});
    if (signal?.aborted) await cancel();
    signal?.addEventListener("abort", cancel, { once: true });
    let child;
    try {
      child = spawn(this.executable, ["request", "--cancel-file", cancelFile], {
        stdio: ["pipe", "pipe", "pipe"], windowsHide: true,
      });
      const exited = new Promise((resolveExit, rejectExit) => {
        child.once("error", rejectExit);
        child.once("close", resolveExit);
      });
      let stderr = "";
      child.stderr.setEncoding("utf8");
      child.stderr.on("data", (chunk) => { stderr += chunk; });
      child.stdin.end(JSON.stringify({ operation, input: resolve(input), password, newPassword, output: output ? resolve(output) : null, compress, renameExisting }) + "\n");
      let result = null;
      let errorMessage = null;
      const lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
      for await (const line of lines) {
        const event = JSON.parse(line);
        if (event.type === "started" && onProgress) onProgress({ processed: 0, total: event.total, percent: 0 });
        else if (event.type === "progress" && onProgress) onProgress(event);
        else if (event.type === "result") result = event;
        else if (event.type === "error") errorMessage = event.message;
      }
      const code = await exited;
      if (errorMessage) throw new BafenError(errorMessage);
      if (code !== 0) throw new BafenError(stderr.trim() || "操作未完成");
      if (!result) throw new BafenError("核心程序未返回处理结果");
      return { filename: result.filename, originalSize: result.originalSize, output: result.output };
    } catch (error) {
      if (error?.code === "ENOENT") throw new BafenError(`无法启动观海·八分核心程序：${this.executable}`);
      throw error;
    } finally {
      signal?.removeEventListener("abort", cancel);
      child?.kill();
      await rm(temporary, { recursive: true, force: true });
    }
  }

  pack(input, password, output = null, options = {}) { return this.execute({ operation: "pack", input, password, output, compress: options.compress, renameExisting: options.renameExisting }, options); }
  unpack(input, password, output = null, options = {}) { return this.execute({ operation: "unpack", input, password, output, renameExisting: options.renameExisting }, options); }
  verify(input, password, options = {}) { return this.execute({ operation: "verify", input, password }, options); }
  migrate(input, password, newPassword, output = null, options = {}) { return this.execute({ operation: "migrate", input, password, newPassword, output, renameExisting: options.renameExisting }, options); }
  recover(input, password, output = null, options = {}) { return this.execute({ operation: "recover", input, password, output }, options); }
}
