/**
 * How big a document is in bytes, as opposed to in whatever the runtime stores it as.
 *
 * `EditorSurface` has a size gate — past a megabyte the buffer opens without a grammar,
 * because a `StreamLanguage` parse can never catch up with the viewport on a file that big
 * and the colour is absent either way. The gate is spelled in *bytes*, which is the unit the
 * file has on disk and the unit the number was chosen in.
 *
 * `source.length` is not that number. A JavaScript string's length is UTF-16 code units, so
 * a file of Cyrillic or CJK prose measures at half to a third of its real size and a 2.5 MB
 * file of it sails under a 1 MB limit — the exact document where the parse is slowest gets
 * the parser it cannot afford, and nothing says so because the symptom is "the colour never
 * arrives", which looks like the gate working.
 *
 * # Why not `new TextEncoder().encode(text).length`
 *
 * It is correct and it allocates the whole encoding — up to three times the document — to
 * throw it away and keep the length. That is 15 MB of garbage on the 5 MB file the milestone
 * tests with, at the moment the user is waiting for a file to appear. [`exceedsBytes`] answers
 * the question the gate actually asks without allocating anything, and usually without
 * looking at the text at all.
 */

/**
 * The number of bytes `text` occupies as UTF-8.
 *
 * Agrees with `TextEncoder`, unpaired surrogates included: WHATWG encoding replaces a lone
 * surrogate with U+FFFD, which is three bytes, and so does the `else` below.
 */
export function utf8ByteLength(text: string): number {
  let bytes = 0
  for (let i = 0; i < text.length; i++) {
    const unit = text.charCodeAt(i)
    if (unit < 0x80) bytes += 1
    else if (unit < 0x800) bytes += 2
    else if (
      unit >= 0xd800 &&
      unit < 0xdc00 &&
      i + 1 < text.length &&
      (text.charCodeAt(i + 1) & 0xfc00) === 0xdc00
    ) {
      // A well-formed surrogate pair is one code point in four bytes, and consumes both units.
      bytes += 4
      i++
    } else bytes += 3
  }
  return bytes
}

/**
 * Whether `text` is larger than `limit` bytes, deciding from its length where it can.
 *
 * Two bounds make the scan unnecessary for almost every document, and both are exact rather
 * than heuristic. Every code point contributes at least one byte per code unit, so the UTF-8
 * size is never *below* the string length; and the worst case is a three-byte BMP character,
 * which is one code unit — an astral character is four bytes across two units and so is
 * cheaper per unit — so the size is never above three times the length.
 *
 * A document therefore only needs counting when its length sits between `limit / 3` and
 * `limit`, and there the count is bounded by `limit` code units. Above and below that band
 * the answer is arithmetic on a number the runtime already has.
 */
export function exceedsBytes(text: string, limit: number): boolean {
  if (text.length > limit) return true
  if (text.length * 3 <= limit) return false
  return utf8ByteLength(text) > limit
}
