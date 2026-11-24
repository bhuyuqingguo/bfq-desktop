import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { BafenCore, BafenError } from "./index.mjs";

const executable = process.env.GUANHAI_CORE;
if (!executable) throw new Error("Set GUANHAI_CORE to the compiled core executable");
const directory = await mkdtemp(join(tmpdir(), "bafen-node-test-"));
const core = new BafenCore({ executable });
try {
  const source = join(directory, "时光样本.txt");
  const archive = join(directory, "样本.bfq");
  const recovered = join(directory, "恢复.txt");
  const expected = Buffer.alloc(120000, 47);
  await writeFile(source, expected);
  const progress = [];
  const result = await core.pack(source, "local-only-password", archive, {
    onProgress: (event) => progress.push(event),
  });
  assert.equal(result.filename, "时光样本.txt");
  assert.ok(progress.length > 0);
  await core.unpack(archive, "local-only-password", recovered);
  assert.deepEqual(await readFile(recovered), expected);
  await assert.rejects(core.unpack(archive, "wrong-password", join(directory, "wrong.txt")), BafenError);

  const controller = new AbortController();
  await assert.rejects(core.pack(source, "password", join(directory, "cancelled.bfq"), {
    signal: controller.signal,
    onProgress: () => controller.abort(),
  }), /取消/);
  console.log("Node adapter: roundtrip, progress, wrong password and cancellation passed");
} finally {
  await rm(directory, { recursive: true, force: true });
}
