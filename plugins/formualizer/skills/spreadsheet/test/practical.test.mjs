import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { recalculateFile } from "../scripts/run.mjs";

test("byte equality stays separate from deferred coverage and legacy current", async t => {
  const dir = await mkdtemp(join(tmpdir(), "practical-recalc-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const input = join(dir, "in.xlsx"), bytes = Buffer.from("valid binding-owned bytes");
  await writeFile(input, bytes);
  let freed = 0;
  const receipt = await recalculateFile({
    recalculateSourceXlsx() {
      return { bytes: new Uint8Array(bytes),
        metadataJson: JSON.stringify({ coverage: "deferred", completed: false, formulaCells: null }),
        free() { freed++; } };
    }
  }, input);
  assert.equal(receipt.bytes_unchanged, true);
  assert.equal(receipt.bytesUnchanged, true);
  assert.equal(receipt.current, true);
  assert.equal(receipt.metadata.coverage, "deferred");
  assert.equal(receipt.metadata.completed, false);
  assert.equal(receipt.metadata.formulaCells, null);
  assert.equal(freed, 1);
});
