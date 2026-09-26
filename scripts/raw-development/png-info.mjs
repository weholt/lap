#!/usr/bin/env node
// Minimal PNG inspector for RAW-development baseline tooling (lap-7f5.2).
//
// Reads PNG header dimensions/bit depth/color type and can sample decoded
// pixels from the baseline exports. Supports the color types the RapidRAW
// export encoder emits (truecolor 8/16-bit, with or without alpha) plus
// palette files are intentionally rejected. No external dependencies.
//
// Usage:
//   node png-info.mjs <file>                     # header info as JSON
//   node png-info.mjs <file> --pixel X,Y         # one decoded pixel as RGB
//   node png-info.mjs <file> --pixel X,Y --json

import { inflateSync } from 'node:zlib'
import { readFileSync } from 'node:fs'

const [file, ...rest] = process.argv.slice(2)
if (!file) {
  console.error('usage: png-info.mjs <file> [--pixel X,Y] [--json]')
  process.exit(2)
}
const flags = new URLSearchParams(
  rest.flatMap((a, i, arr) => (a.startsWith('--') && !a.includes(',') ? [[a.slice(2), arr[i + 1]]] : [])),
)

const buf = readFileSync(file)
const bne = (o) => buf.readUInt32BE(o)
if (buf.readUInt32BE(0) !== 0x89504e47) throw new Error('not a PNG')
const width = bne(16)
const height = bne(20)
const bitDepth = buf[24]
const colorType = buf[25]

const channelsByColorType = { 0: 1, 2: 3, 3: null, 4: 2, 6: 4 }
const channels = channelsByColorType[colorType]
if (channels === null) throw new Error('palette PNG not supported')
if (bitDepth !== 8 && bitDepth !== 16) throw new Error(`unsupported bit depth ${bitDepth}`)

const info = { file, width, height, bitDepth, colorType }

function decode() {
  const bytesPerPixel = channels * (bitDepth / 8)
  const stride = width * bytesPerPixel
  // Concatenate all IDAT chunks.
  const idat = []
  for (let o = 8; o < buf.length; ) {
    const len = bne(o)
    const type = buf.toString('ascii', o + 4, o + 8)
    if (type === 'IDAT') idat.push(buf.subarray(o + 8, o + 8 + len))
    o += 12 + len
    if (type === 'IEND') break
  }
  const raw = inflateSync(Buffer.concat(idat))
  const out = Buffer.alloc(height * stride)
  let pos = 0
  const prev = Buffer.alloc(stride)
  const cur = Buffer.alloc(stride)
  for (let y = 0; y < height; y++) {
    const filter = raw[pos++]
    raw.copy(cur, 0, pos, pos + stride)
    pos += stride
    switch (filter) {
      case 0: break
      case 1:
        for (let i = bytesPerPixel; i < stride; i++) cur[i] = (cur[i] + cur[i - bytesPerPixel]) & 0xff
        break
      case 2:
        for (let i = 0; i < stride; i++) cur[i] = (cur[i] + prev[i]) & 0xff
        break
      case 3:
        for (let i = 0; i < stride; i++) {
          const left = i >= bytesPerPixel ? cur[i - bytesPerPixel] : 0
          cur[i] = (cur[i] + ((left + prev[i]) >> 1)) & 0xff
        }
        break
      case 4: {
        const paeth = (a, b, c) => {
          const p = a + b - c
          const pa = Math.abs(p - a)
          const pb = Math.abs(p - b)
          const pc = Math.abs(p - c)
          if (pa <= pb && pa <= pc) return a
          if (pb <= pc) return b
          return c
        }
        for (let i = 0; i < stride; i++) {
          const left = i >= bytesPerPixel ? cur[i - bytesPerPixel] : 0
          const up = prev[i]
          const ul = i >= bytesPerPixel ? prev[i - bytesPerPixel] : 0
          cur[i] = (cur[i] + paeth(left, up, ul)) & 0xff
        }
        break
      }
      default:
        throw new Error(`unsupported PNG filter ${filter}`)
    }
    cur.copy(out, y * stride)
    cur.copy(prev)
    raw.copy(cur, 0, pos, pos) // keep cur mutable for the next row
  }
  return out
}

const pixelArg = flags.get('pixel')
if (pixelArg) {
  const [x, y] = pixelArg.split(',').map(Number)
  if (!Number.isInteger(x) || !Number.isInteger(y)) throw new Error('--pixel expects X,Y')
  if (x < 0 || y < 0 || x >= width || y >= height) throw new Error('pixel out of bounds')
  const pixels = decode()
  const bytesPerPixel = channels * (bitDepth / 8)
  const base = (y * width + x) * bytesPerPixel
  const rgb = []
  for (let c = 0; c < 3; c++) {
    if (bitDepth === 16) rgb.push(pixels.readUInt16BE(base + c * 2))
    else rgb.push(pixels[base + c])
  }
  info.pixel = { x, y, rgb, bitDepth }
}

console.log(JSON.stringify(info, null, flags.has('json') ? 2 : 0))
