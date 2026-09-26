#!/usr/bin/env node
// Deterministic derived-fixture tool (lap-7f5.2 / TASK-102).
//
// Creates the orientation corpus entry by copying a CC0 linear DNG and
// rewriting its TIFF Orientation tag (274) from 1 (Normal) to a target
// display rotation. Only the two bytes of the inline SHORT value change:
// sensor data, structure, and all other tags stay byte-identical, so the
// derived file inherits the source's CC0-1.0 permitted use.
//
// The tool refuses unknown structures instead of guessing: the tag must be
// present in IFD0 as an inline SHORT with the expected current value.
//
// Usage:
//   node scripts/raw-development/make-orientation-variant.mjs \
//     tests/fixtures/raw-development/corpus/<src>.DNG \
//     tests/fixtures/raw-development/corpus/<out>.DNG [orientation=6]

import { createHash } from 'node:crypto'
import { readFileSync, writeFileSync } from 'node:fs'

const [input, output, orientationArg] = process.argv.slice(2)
if (!input || !output) {
  console.error('usage: make-orientation-variant.mjs <input> <output> [orientation=6]')
  process.exit(2)
}
const orientation = Number(orientationArg ?? 6)
if (![5, 6, 7, 8].includes(orientation)) {
  console.error(`orientation must be one of 5,6,7,8 (got ${orientation})`)
  process.exit(2)
}

const bytes = readFileSync(input)
const sha256 = (buf) => createHash('sha256').update(buf).digest('hex')

const byteAt = (i) => bytes[i]
if (byteAt(0) !== 0x49 || byteAt(1) !== 0x49) {
  failUnlessLittleEndian()
}
function failUnlessLittleEndian() {
  if (byteAt(0) !== 0x4d || byteAt(1) !== 0x4d) {
    throw new Error('not a TIFF (expected II or MM magic)')
  }
  throw new Error('big-endian TIFF not handled by this deterministic tool')
}
const u16 = (o) => bytes.readUInt16LE(o)
const u32 = (o) => bytes.readUInt32LE(o)
if (u16(2) !== 42) throw new Error('not a classic TIFF (magic != 42)')

const ifd0 = u32(4)
const entryCount = u16(ifd0)
let found = -1
for (let i = 0; i < entryCount; i++) {
  const entry = ifd0 + 2 + i * 12
  if (u16(entry) === 274) {
    found = entry
    break
  }
}
if (found < 0) throw new Error('Orientation tag (274) not present in IFD0')
if (u16(found + 2) !== 3 || u32(found + 4) !== 1) {
  throw new Error('Orientation tag is not an inline SHORT(1); refusing to patch')
}
const current = u16(found + 8)
if (current !== 1) {
  throw new Error(`expected current Orientation value 1, found ${current}`)
}

const derived = Buffer.from(bytes)
derived.writeUInt16LE(orientation, found + 8)
writeFileSync(output, derived)

const report = {
  schema: 'lap-raw-orientation-variant/v1',
  input: { path: input, sha256: sha256(bytes) },
  output: { path: output, sha256: sha256(derived), bytes: derived.length },
  transformation: `TIFF tag 274 (Orientation): 1 -> ${orientation}; no other bytes changed`,
  orientation,
}
console.log(JSON.stringify(report, null, 2))
