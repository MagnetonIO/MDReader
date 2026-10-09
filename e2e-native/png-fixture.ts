import { inflateSync } from 'node:zlib';

function crc32(bytes: Buffer): number {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

/** Fixture preflight only; product image validation still runs in the native owner. */
export function checkPngFixture(bytes: Buffer): Buffer {
  const fail = (): never => { throw new Error('Public PNG fixture is corrupt or unsupported. Fix the fixture before starting device acceptance.'); };
  if (bytes.length < 45 || bytes.length > 4 * 1024 * 1024 || !bytes.subarray(0, 8).equals(Buffer.from('89504e470d0a1a0a', 'hex'))) fail();
  let offset = 8; let width = 0; let height = 0; let channels = 0; let ended = false;
  const imageData: Buffer[] = [];
  while (offset + 12 <= bytes.length) {
    const length = bytes.readUInt32BE(offset); const end = offset + 12 + length;
    if (end > bytes.length || crc32(bytes.subarray(offset + 4, end - 4)) !== bytes.readUInt32BE(end - 4)) fail();
    const kind = bytes.toString('ascii', offset + 4, offset + 8);
    if (offset === 8 && kind !== 'IHDR') fail();
    if (kind === 'IHDR') {
      if (offset !== 8 || length !== 13) fail();
      width = bytes.readUInt32BE(offset + 8); height = bytes.readUInt32BE(offset + 12);
      const depth = bytes[offset + 16]; const color = bytes[offset + 17];
      if (!width || !height || width > 4096 || height > 4096 || width * height > 4_000_000 || depth !== 8 || ![2, 6].includes(color!) || bytes[offset + 18] !== 0 || bytes[offset + 19] !== 0 || bytes[offset + 20] !== 0) fail();
      channels = color === 2 ? 3 : 4;
    } else if (kind === 'IDAT') imageData.push(bytes.subarray(offset + 8, end - 4));
    else if (kind === 'acTL') fail();
    else if (kind === 'IEND') {
      if (length !== 0 || end !== bytes.length) fail();
      ended = true; break;
    }
    offset = end;
  }
  if (!ended || !imageData.length || !channels) fail();
  const row = 1 + width * channels;
  let decoded: Buffer;
  try { decoded = inflateSync(Buffer.concat(imageData), { maxOutputLength: row * height }); }
  catch { return fail(); }
  if (decoded.length !== row * height) fail();
  for (let index = 0; index < height; index++) if (decoded[index * row]! > 4) fail();
  return bytes;
}
