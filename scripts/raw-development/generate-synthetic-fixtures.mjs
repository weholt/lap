#!/usr/bin/env node
// Deterministic synthetic edge-case fixture generator (lap-7f5.2 / TASK-102).
//
// Generates tiny uncompressed linear DNGs (LinearRaw, 3x16-bit RGB, little
// endian TIFF) with exactly known pixel values, plus deliberately broken
// failure inputs. These complement the real CC0 corpus for numerical and
// failure tests; they do NOT establish camera-RAW parity (spec P4).
//
// Output is byte-for-byte deterministic: no timestamps, no randomness, fixed
// layout. Committed files must always match synthetic-manifest.json.
//
// Usage:
//   node scripts/raw-development/generate-synthetic-fixtures.mjs [--out DIR]
//     --out writes to DIR/synthetic instead of the corpus (used by tests to
//     prove determinism); the manifest is only written without --out.

import { createHash } from 'node:crypto'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..')
const args = process.argv.slice(2)
let outDir = null
for (let i = 0; i < args.length; i++) {
  if (args[i] === '--out' && args[i + 1]) outDir = args[i + 1]
}
const root = outDir ?? path.join(repoRoot, 'tests/fixtures/raw-development')
const syntheticRoot = path.join(root, 'synthetic')
mkdirSync(syntheticRoot, { recursive: true })

// ---------------------------------------------------------------------------
// Minimal linear DNG writer (classic TIFF, single strip in IFD0).
// ---------------------------------------------------------------------------

const TAG = {
  NewSubfileType: 254,
  ImageWidth: 256,
  ImageLength: 257,
  BitsPerSample: 258,
  Compression: 259,
  PhotometricInterpretation: 262,
  Make: 271,
  Model: 272,
  StripOffsets: 273,
  Orientation: 274,
  SamplesPerPixel: 277,
  RowsPerStrip: 278,
  StripByteCounts: 279,
  PlanarConfiguration: 284,
  SampleFormat: 339,
  DNGVersion: 50706,
  DNGBackwardVersion: 50707,
  UniqueCameraModel: 50708,
  BlackLevel: 50714,
  WhiteLevel: 50717,
  ColorMatrix1: 50721,
  CalibrationIlluminant1: 50778,
}
const TYPE = { SHORT: 3, LONG: 4, RATIONAL: 5, ASCII: 2, BYTE: 1 }
const TYPE_SIZE = { 1: 1, 2: 1, 3: 2, 4: 4, 5: 8 }

class ByteWriter {
  constructor() {
    this.chunks = []
    this.length = 0
  }
  push(buf) {
    this.chunks.push(buf)
    this.length += buf.length
    return this
  }
  u8(v) {
    return this.push(Buffer.from([v & 0xff]))
  }
  u16(v) {
    return this.push(Buffer.from([(v & 0xff), (v >> 8) & 0xff]))
  }
  u32(v) {
    return this.push(Buffer.from([v & 0xff, (v >> 8) & 0xff, (v >> 16) & 0xff, (v >> 24) & 0xff]))
  }
  ascii(s) {
    const buf = Buffer.alloc(s.length + 1)
    buf.write(s, 'latin1')
    return this.push(buf)
  }
  toBuffer() {
    return Buffer.concat(this.chunks, this.length)
  }
}

/**
 * Build an uncompressed linear DNG. `pixels(w, h)` returns the RGB payload
 * (Uint16Array length w*h*3, interleaved). `orientation` is the TIFF
 * Orientation tag value (1 or 6).
 */
function buildLinearDng({ width, height, orientation = 1, pixels }) {
  const payload = Buffer.alloc(width * height * 3 * 2)
  const values = pixels(width, height)
  if (values.length !== width * height * 3) throw new Error('pixel callback produced wrong length')
  for (let i = 0; i < values.length; i++) payload.writeUInt16LE(values[i] & 0xffff, i * 2)

  const external = [] // { bytes: Buffer } laid out after the IFD, in order
  const entries = [] // { tag, type, count, inlineValue: Buffer|null, externalIndex }
  const addInline = (tag, type, count, valueBuf) => {
    entries.push({ tag, type, count, value: valueBuf })
  }
  const addExternal = (tag, type, count, valueBuf) => {
    entries.push({ tag, type, count, value: valueBuf, externalIndex: external.length })
    external.push(valueBuf)
  }
  const shorts = (arr) => {
    const buf = Buffer.alloc(arr.length * 2)
    arr.forEach((v, i) => buf.writeUInt16LE(v, i * 2))
    return buf
  }
  const rationals = (arr) => {
    const buf = Buffer.alloc(arr.length * 8)
    arr.forEach((v, i) => {
      buf.writeInt32LE(v.n, i * 8)
      buf.writeInt32LE(v.d, i * 8 + 4)
    })
    return buf
  }

  addInline(TAG.NewSubfileType, TYPE.LONG, 1, Buffer.from([0, 0, 0, 0]))
  addInline(TAG.ImageWidth, TYPE.LONG, 1, (() => { const b = Buffer.alloc(4); b.writeUInt32LE(width); return b })())
  addInline(TAG.ImageLength, TYPE.LONG, 1, (() => { const b = Buffer.alloc(4); b.writeUInt32LE(height); return b })())
  addExternal(TAG.BitsPerSample, TYPE.SHORT, 3, shorts([16, 16, 16]))
  addInline(TAG.Compression, TYPE.SHORT, 1, shorts([1]))
  addInline(TAG.PhotometricInterpretation, TYPE.SHORT, 1, shorts([34892])) // LinearRaw
  addInline(TAG.Make, TYPE.ASCII, 4, (() => { const b = Buffer.alloc(4); b.write('Lap\0', 'latin1'); return b })())
  addExternal(TAG.Model, TYPE.ASCII, 10, (() => { const b = Buffer.alloc(10); b.write('SynthEdge\0', 'latin1'); return b })())
  const stripOffsetsEntry = { tag: TAG.StripOffsets, type: TYPE.LONG, count: 1, value: Buffer.alloc(4) }
  entries.push(stripOffsetsEntry)
  addInline(TAG.Orientation, TYPE.SHORT, 1, shorts([orientation]))
  addInline(TAG.SamplesPerPixel, TYPE.SHORT, 1, shorts([3]))
  addInline(TAG.RowsPerStrip, TYPE.LONG, 1, (() => { const b = Buffer.alloc(4); b.writeUInt32LE(height); return b })())
  addInline(TAG.StripByteCounts, TYPE.LONG, 1, (() => { const b = Buffer.alloc(4); b.writeUInt32LE(payload.length); return b })())
  addInline(TAG.PlanarConfiguration, TYPE.SHORT, 1, shorts([1]))
  addExternal(TAG.SampleFormat, TYPE.SHORT, 3, shorts([1, 1, 1])) // unsigned int
  addInline(TAG.DNGVersion, TYPE.BYTE, 4, Buffer.from([1, 4, 0, 0]))
  addInline(TAG.DNGBackwardVersion, TYPE.BYTE, 4, Buffer.from([1, 1, 0, 0]))
  addExternal(TAG.UniqueCameraModel, TYPE.ASCII, 25, (() => { const b = Buffer.alloc(25); b.write('LapSynthetic EdgeCaseGen\0', 'latin1'); return b })())
  addExternal(TAG.BlackLevel, TYPE.SHORT, 3, shorts([0, 0, 0]))
  addInline(TAG.WhiteLevel, TYPE.SHORT, 1, shorts([65535]))
  addExternal(
    TAG.ColorMatrix1,
    TYPE.RATIONAL,
    9,
    rationals([
      { n: 1, d: 1 }, { n: 0, d: 1 }, { n: 0, d: 1 },
      { n: 0, d: 1 }, { n: 1, d: 1 }, { n: 0, d: 1 },
      { n: 0, d: 1 }, { n: 0, d: 1 }, { n: 1, d: 1 },
    ]),
  )
  addInline(TAG.CalibrationIlluminant1, TYPE.SHORT, 1, shorts([23])) // D50

  entries.sort((a, b) => a.tag - b.tag)

  const ifdOffset = 8
  const ifdSize = 2 + entries.length * 12 + 4
  let cursor = ifdOffset + ifdSize
  const externalOffsets = external.map((buf) => {
    const offset = cursor
    cursor += buf.length
    return offset
  })
  const payloadOffset = cursor
  stripOffsetsEntry.value.writeUInt32LE(payloadOffset)

  const out = new ByteWriter()
  out.u8(0x49).u8(0x49).u16(42).u32(ifdOffset)
  out.u16(entries.length)
  for (const e of entries) {
    out.u16(e.tag).u16(e.type).u32(e.count)
    let valueField
    if (e.externalIndex !== undefined) valueField = externalOffsets[e.externalIndex]
    else if (e.value.length === 4) valueField = e.value.readUInt32LE(0)
    else if (e.value.length === 2) valueField = e.value.readUInt16LE(0)
    else if (e.value.length < 4) {
      valueField = 0
      for (let i = 0; i < e.value.length; i++) valueField |= e.value[i] << (8 * i)
    } else throw new Error(`tag ${e.tag} marked inline but needs ${e.value.length} bytes`)
    out.u32(valueField)
  }
  out.u32(0) // next IFD
  for (const buf of external) out.push(buf)
  if (out.length !== payloadOffset) {
    throw new Error(`layout drift: expected payload at ${payloadOffset}, at ${out.length}`)
  }
  out.push(payload)
  return out.toBuffer()
}

// ---------------------------------------------------------------------------
// Fixture definitions (exact, documented values).
// ---------------------------------------------------------------------------

const fixtures = [
  {
    file: 'synthetic/dng-linear-gradient-64x48.dng',
    purpose: 'numerical',
    brief: '64x48 linear RGB DNG; R=x/(w-1), G=y/(h-1), B=(x+y)/(w+h-2), scaled to 65535',
    build: () => buildLinearDng({
      width: 64,
      height: 48,
      orientation: 1,
      pixels: (w, h) => {
        const px = new Uint16Array(w * h * 3)
        let i = 0
        for (let y = 0; y < h; y++) {
          for (let x = 0; x < w; x++) {
            px[i++] = Math.round((x / (w - 1)) * 65535)
            px[i++] = Math.round((y / (h - 1)) * 65535)
            px[i++] = Math.round(((x + y) / (w + h - 2)) * 65535)
          }
        }
        return px
      },
    }),
  },
  {
    file: 'synthetic/dng-linear-orientation6-64x48.dng',
    purpose: 'numerical',
    brief: 'Same gradient as dng-linear-gradient-64x48.dng with TIFF Orientation=6; engine must apply the display rotation',
    build: () => buildLinearDng({
      width: 64,
      height: 48,
      orientation: 6,
      pixels: (w, h) => {
        const px = new Uint16Array(w * h * 3)
        let i = 0
        for (let y = 0; y < h; y++) {
          for (let x = 0; x < w; x++) {
            px[i++] = Math.round((x / (w - 1)) * 65535)
            px[i++] = Math.round((y / (h - 1)) * 65535)
            px[i++] = Math.round(((x + y) / (w + h - 2)) * 65535)
          }
        }
        return px
      },
    }),
  },
  {
    file: 'synthetic/dng-highlight-clipped-64x48.dng',
    purpose: 'numerical',
    brief: 'All channels at WhiteLevel 65535 except a 4x4 near-white patch at 65530; exercises highlight rolloff/clipping math',
    build: () => buildLinearDng({
      width: 64,
      height: 48,
      orientation: 1,
      pixels: (w, h) => {
        const px = new Uint16Array(w * h * 3).fill(65535)
        for (let y = 8; y < 12; y++) {
          for (let x = 8; x < 12; x++) {
            const i = (y * w + x) * 3
            px[i] = 65530
            px[i + 1] = 65530
            px[i + 2] = 65530
          }
        }
        return px
      },
    }),
  },
  {
    file: 'synthetic/dng-linear-wide-5000x64.dng',
    purpose: 'numerical',
    brief: '5000x64 linear gradient (width > 4096 px) for geometry/downscale edge cases at a bounded size',
    build: () => buildLinearDng({
      width: 5000,
      height: 64,
      orientation: 1,
      pixels: (w, h) => {
        const px = new Uint16Array(w * h * 3)
        let i = 0
        for (let y = 0; y < h; y++) {
          for (let x = 0; x < w; x++) {
            px[i++] = Math.round((x / (w - 1)) * 65535)
            px[i++] = Math.round((y / (h - 1)) * 65535)
            px[i++] = 0
          }
        }
        return px
      },
    }),
  },
  {
    file: 'synthetic/dng-extreme-constants-8x2.dng',
    purpose: 'numerical',
    brief: '8x2 exact constants incl. 0, 1, 65534, 65535 per channel for boundary rounding checks',
    build: () => buildLinearDng({
      width: 8,
      height: 2,
      orientation: 1,
      pixels: (w, h) => {
        const row0 = [0, 1, 2, 32767, 32768, 65533, 65534, 65535]
        const row1 = [65535, 65534, 40000, 40000, 40000, 255, 256, 1000]
        const px = new Uint16Array(w * h * 3)
        let i = 0
        for (let x = 0; x < w; x++) {
          px[i++] = row0[x]
          px[i++] = row0[(x + 3) % 8]
          px[i++] = row0[(x + 5) % 8]
        }
        for (let x = 0; x < w; x++) {
          px[i++] = row1[x]
          px[i++] = row1[(x + 1) % 8]
          px[i++] = row1[(x + 6) % 8]
        }
        return px
      },
    }),
  },
]

const failureSources = () => ({
  gradient: fixtures[0].build(),
})

const failureFixtures = [
  {
    file: 'synthetic/truncated-linear.dng',
    purpose: 'failure',
    brief: 'First 96 bytes of dng-linear-gradient-64x48.dng: valid header, cut before the IFD body completes',
    build: (src) => src.gradient.subarray(0, 96),
  },
  {
    file: 'synthetic/header-only-linear.dng',
    purpose: 'failure',
    brief: 'TIFF header (8 bytes) only; IFD offset points past EOF',
    build: () => Buffer.from([0x49, 0x49, 42, 0, 8, 0, 0, 0]),
  },
  {
    file: 'synthetic/wrong-tiff-magic.dng',
    purpose: 'numerical',
    brief: 'Gradient DNG with TIFF magic changed 42->43. Measured engine behavior at the pinned revision: rawler tolerates the wrong magic and renders byte-identically to dng-linear-gradient-64x48.dng; the baseline pins this. A future decoder must either keep this behavior or change the output explicitly (never silently)',
    build: (src) => {
      const copy = Buffer.from(src.gradient)
      copy[2] = 43
      return copy
    },
  },
  {
    file: 'synthetic/not-a-raw.dng',
    purpose: 'failure',
    brief: 'ASCII text with a .dng extension; must fail decode explicitly, never fall through silently',
    build: () => Buffer.from('This is not a RAW file. It only has a .dng extension.\n', 'latin1'),
  },
  {
    file: 'synthetic/empty-raw.dng',
    purpose: 'failure',
    brief: 'Zero-byte file; the emptiest possible failure input',
    build: () => Buffer.alloc(0),
  },
]

const sha256 = (buf) => createHash('sha256').update(buf).digest('hex')
const written = []
for (const fixture of [...fixtures, ...failureFixtures]) {
  const bytes = fixture.build(failureSources())
  const target = path.join(root, fixture.file)
  writeFileSync(target, bytes)
  written.push({
    path: fixture.file,
    bytes: bytes.length,
    sha256: sha256(bytes),
    purpose: fixture.purpose,
    brief: fixture.brief,
  })
  console.log(`${fixture.file}  ${bytes.length} bytes  ${sha256(bytes)}`)
}

if (!outDir) {
  const manifest = {
    schema: 'lap-raw-synthetic/v1',
    issue: 'lap-7f5.2',
    generator: 'scripts/raw-development/generate-synthetic-fixtures.mjs',
    policy: 'Deterministic byte-for-byte output; no timestamps or randomness. Synthetic fixtures complement the real CC0 corpus for numerical and failure tests and do not establish camera-RAW parity (spec P4).',
    files: written,
  }
  const manifestPath = path.join(syntheticRoot, 'synthetic-manifest.json')
  writeFileSync(manifestPath, JSON.stringify(manifest, null, 2) + '\n')
  console.log(`manifest: ${manifestPath}`)
} else {
  // Determinism mode: emit the checksums as JSON for comparison.
  const summaryPath = path.join(syntheticRoot, 'determinism-summary.json')
  writeFileSync(summaryPath, JSON.stringify(written, null, 2) + '\n')
  console.log(`determinism summary: ${summaryPath}`)
}
