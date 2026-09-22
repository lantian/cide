export interface LogStyle {
  color?: string
  backgroundColor?: string
  fontWeight?: number
  fontStyle?: string
  textDecoration?: string
  opacity?: number
}
export interface LogSpan {
  text: string
  style: LogStyle
}
export interface LogLine {
  kind: 'line'
  number: number
  spans: LogSpan[]
  text: string
}
export interface LogSection {
  kind: 'section'
  id: string
  title: string
  collapsed: boolean
  duration?: number
  start: number
  children: LogEntry[]
}
export type LogEntry = LogLine | LogSection
const colors = [
  '#4b4b4b',
  '#d63b3b',
  '#299647',
  '#ad7900',
  '#397ddd',
  '#ab55bd',
  '#1696a3',
  '#c6c6c6',
  '#808080',
  '#ef6868',
  '#64b77b',
  '#dfb551',
  '#74a1e5',
  '#cd84d6',
  '#61bbc7',
  '#f5f5f5',
]
function color(index: number) {
  if (index < 16) return colors[index]
  if (index >= 232) {
    const v = 8 + (index - 232) * 10
    return `rgb(${v},${v},${v})`
  }
  const n = index - 16,
    levels = [0, 95, 135, 175, 215, 255]
  return `rgb(${levels[Math.floor(n / 36)]},${levels[Math.floor(n / 6) % 6]},${levels[n % 6]})`
}
/** Parse terminal SGR as data, never as HTML. Retain styling across log lines. */
export function parseJobLog(raw: string): LogEntry[] {
  const root: LogEntry[] = [],
    stack: LogSection[] = []
  let style: LogStyle = {}
  const append = (entry: LogEntry) =>
    (stack.at(-1)?.children ?? root).push(entry)
  const ansi =
    /\x1b\[([0-?]*)([ -/]*)([@-~])|\x1b\](?:[^\x07\x1b]|\x1b(?!\\))*(?:\x07|\x1b\\)|\r/g
  raw.split('\n').forEach((original, index) => {
    let source = original
    const section =
      /section_(start|end):(\d+):([\w.:-]+)(\[[^\]]*\])?\r(?:\x1b\[0K)?/.exec(
        source,
      )
    if (section) {
      source = source.slice(section.index + section[0].length)
      if (section[1] === 'end') {
        const at = stack.findLastIndex((s) => s.id === section[3])
        if (at >= 0) {
          stack[at]!.duration = Math.max(
            0,
            Number(section[2]) - stack[at]!.start,
          )
          stack.splice(at)
        }
        if (!source) return
      } else {
        const item: LogSection = {
          kind: 'section',
          id: section[3]!,
          title: source.replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '') || section[3]!,
          start: Number(section[2]),
          collapsed: section[4]?.includes('collapsed=true') ?? false,
          children: [],
        }
        append(item)
        stack.push(item)
        // Parse title SGR as well: runners can leave a style active for the section body.
      }
    }
    source = source.replace(/\r$/, '')
    const spans: LogSpan[] = []
    let offset = 0
    const push = (text: string) => {
      if (text) spans.push({ text, style: { ...style } })
    }
    for (const match of source.matchAll(ansi)) {
      push(source.slice(offset, match.index))
      offset = match.index + match[0].length
      if (match[0] === '\r') {
        spans.length = 0
        continue
      }
      if (match[3] !== 'm') continue
      const codes = (match[1] || '0').split(';').map(Number)
      for (let i = 0; i < codes.length; i++) {
        const c = codes[i]!
        if (c === 0) style = {}
        else if (c === 1) style.fontWeight = 700
        else if (c === 2) style.opacity = 0.65
        else if (c === 3) style.fontStyle = 'italic'
        else if (c === 4) style.textDecoration = 'underline'
        else if (c === 22) {
          delete style.fontWeight
          delete style.opacity
        } else if (c === 23) delete style.fontStyle
        else if (c === 24) delete style.textDecoration
        else if (c === 39) delete style.color
        else if (c === 49) delete style.backgroundColor
        else if ((c >= 30 && c <= 37) || (c >= 90 && c <= 97))
          style.color = colors[c >= 90 ? c - 90 + 8 : c - 30]!
        else if ((c >= 40 && c <= 47) || (c >= 100 && c <= 107))
          style.backgroundColor = colors[c >= 100 ? c - 100 + 8 : c - 40]!
        else if (c === 38 || c === 48) {
          let value: string | undefined
          if (
            codes[i + 1] === 5 &&
            codes[i + 2] !== undefined &&
            codes[i + 2]! >= 0 &&
            codes[i + 2]! <= 255
          ) {
            value = color(codes[i + 2]!)
            i += 2
          } else if (
            codes[i + 1] === 2 &&
            codes.slice(i + 2, i + 5).length === 3 &&
            codes.slice(i + 2, i + 5).every((v) => v >= 0 && v <= 255)
          ) {
            value = `rgb(${codes.slice(i + 2, i + 5).join(',')})`
            i += 4
          }
          if (value) style[c === 38 ? 'color' : 'backgroundColor'] = value
        }
      }
    }
    push(source.slice(offset))
    if (!section || section[1] === 'end')
      append({
        kind: 'line',
        number: index + 1,
        spans,
        text: spans.map((s) => s.text).join(''),
      })
  })
  return root
}
