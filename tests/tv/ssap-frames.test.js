const fs = require("fs"), assert = require("assert"), Crypto = require("crypto");
const src = fs.readFileSync(process.argv[2], "utf8");
const start = src.indexOf("\tfunction UnframeData");
const end = src.indexOf("\n\t}", src.indexOf("\tfunction Unmask")) + 3;
eval(src.slice(start, end)); // defines UnframeData, FrameData, Unmask

// Decode as the TV's server would: the client's frames must be masked
function serverRead(buf) {
  const out = [];
  const rest = UnframeData(buf, (frame, fin, r1, r2, r3, op, mask, data) => {
    assert.ok(mask, "client frames are masked");
    out.push({ fin, op, text: Buffer.from(Unmask(mask, Buffer.from(data))).toString("utf8") });
  });
  return { out, rest };
}
for (const n of [0, 5, 125, 126, 300, 65535, 65536, 70000]) {
  const text = "x".repeat(n);
  const f = FrameData(text, true, false, false, false, 0x1);
  const want = n > 0xFFFF ? 127 : n > 125 ? 126 : n;
  assert.strictEqual(f[1] & 0x7F, want, `length marker for ${n}`);
  if (n > 0xFFFF) assert.strictEqual(f.readUInt32BE(2), 0, "64-bit length: high half is zero");
  const { out, rest } = serverRead(f);
  assert.strictEqual(out.length, 1); assert.strictEqual(out[0].text, text); assert.strictEqual(rest.length, 0);
}
// Two frames delivered in every possible split, the way TCP may cut them
const a = FrameData(JSON.stringify({ type: "hello" }), true, false, false, false, 0x1);
const b = FrameData("y".repeat(200), true, false, false, false, 0x1);
const both = Buffer.concat([a, b]);
for (let cut = 0; cut <= both.length; cut++) {
  let pending = Buffer.alloc(0), got = [];
  for (const chunk of [both.slice(0, cut), both.slice(cut)]) {
    pending = UnframeData(Buffer.concat([pending, chunk]), (f, fin, r1, r2, r3, op, mask, data) => got.push(Buffer.from(Unmask(mask, Buffer.from(data))).toString()));
  }
  assert.deepStrictEqual(got, ['{"type":"hello"}', "y".repeat(200)], `split at ${cut}`);
  assert.strictEqual(pending.length, 0);
}
// A pong carries the ping's binary payload
const pong = FrameData(Buffer.from([1, 2, 255]), true, false, false, false, 0xA);
UnframeData(pong, (f, fin, r1, r2, r3, op, mask, data) => { assert.strictEqual(op, 0xA); assert.deepStrictEqual([...Unmask(mask, Buffer.from(data))], [1, 2, 255]); });
console.log("SSAP framing: all checks passed");
