/**
 * Which files carry a Compose stack, and what can be asked of one. (M48)
 *
 * # Why this is here and not in the docker panel
 *
 * Because the panel is not the surface that uses it. Three of the four places a compose action is
 * reachable — the file tree's context menu, the editor's gutter, the command palette — know about
 * a *file* and nothing about Docker, and none of them may import the panel. What they share is
 * this table.
 *
 * # Import-free on purpose
 *
 * `ui/scripts/check-docker.mjs` compiles this module standalone and drives it, which is how
 * [`isComposeFile`] is held to the same answers as `cide_docker::compose::is_compose_file` — the
 * two are consulted at different ends of one gesture (this one decides whether to *offer* the
 * action, that one is what actually runs), and a disagreement is a menu entry that appears and
 * then refuses, or one that never appears at all. Keep it importing nothing.
 */

/** The verbs, spelled as `cide_ipc::docker::ComposeAction` serialises them. */
export type ComposeVerb = 'up' | 'down' | 'restart' | 'recreate' | 'build' | 'pull'

/**
 * Every verb, in the order a menu draws them.
 *
 * `up` first because it is what somebody looking at a compose file almost always wants, `down`
 * beside it, then the two that re-apply a file (`restart` re-runs the containers as they are,
 * `recreate` throws them away and builds them from the file again — the one that answers "I just
 * edited this"), then the two long ones.
 */
export const COMPOSE_VERBS: readonly ComposeVerb[] = [
  'up',
  'down',
  'restart',
  'recreate',
  'build',
  'pull',
]

/**
 * What each verb is called, and it is never the bare word.
 *
 * "Up" alone in a file tree's context menu, next to *Rename* and *Delete*, reads as a direction
 * rather than a command. Every row says `Compose` so the menu is self-describing wherever it is
 * spliced in — and the palette, which has no surrounding context at all, needs it most.
 */
export const COMPOSE_LABEL: Record<ComposeVerb, string> = {
  up: 'Compose Up',
  down: 'Compose Down',
  restart: 'Compose Restart',
  recreate: 'Compose Recreate',
  build: 'Compose Build',
  pull: 'Compose Pull',
}

/**
 * Whether a file name is one Compose would read.
 *
 * A **name** test and not a parse, `cide_docker::compose::is_compose_file`'s reason: the surfaces
 * that ask, ask about every file the user is looking at, and reading each one to find a
 * `services:` key would be a filesystem read per tree row. Being wrong this way costs one menu
 * entry that Compose then refuses in its own words; parsing would cost a stuttering tree.
 *
 * The set is Compose's own plus the unofficial spellings people actually have on disk.
 * Case-insensitive: the tree carries the name as the filesystem spells it, and macOS does not care.
 *
 * **Any dotted segment, not just the first.** This tested only the leading segment until M51,
 * which was enough for `compose.yaml` and `docker-compose.yml` and refused `docker.compose.yaml`,
 * whose first segment is `docker`. The failure is silent in the worst way — Compose reads the file
 * happily and cide simply draws no gutter marker, with nothing saying why.
 *
 * Still refuses `composer.yaml`, `decompose.yaml` and `values.yaml`: none has a *segment* equal to
 * `compose`, which is why the test is on a whole segment rather than a prefix.
 */
export function isComposeFile(name: string): boolean {
  const lower = name.toLowerCase()
  const stem = lower.endsWith('.yaml')
    ? lower.slice(0, -'.yaml'.length)
    : lower.endsWith('.yml')
      ? lower.slice(0, -'.yml'.length)
      : null
  if (stem === null) return false
  return stem
    .split('.')
    .some((segment) => segment === 'compose' || segment.endsWith('-compose'))
}

/**
 * The base name of a path, as the container-free half of this module can spell it.
 *
 * Always `/` **and** `\`: this one runs against host paths, unlike `dockerFilesModel.join`, and a
 * Windows path reaching here would otherwise be one long "file name" that matches nothing.
 */
export function baseName(path: string): string {
  const at = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return at === -1 ? path : path.slice(at + 1)
}

/** Whether a whole path names a compose file. The tree and the editor both hold paths. */
export function isComposePath(path: string): boolean {
  return isComposeFile(baseName(path))
}

/** One place a compose file offers a run: the whole file, or one service in it. */
export interface ComposeTarget {
  /** 1-based, as CodeMirror numbers lines. */
  readonly line: number
  /** `null` for the whole file. */
  readonly service: string | null
}

/**
 * Where a compose file's gutter draws a run marker. (M48)
 *
 * # Why this scans rather than parses
 *
 * Because it runs on a buffer that is **being edited**, and a YAML parser's answer to a half-typed
 * document is an exception. A gutter that vanished on the keystroke between `web:` and its body
 * would be worse than one that is occasionally generous — and generous is the only way it can be
 * wrong: an extra marker offers a `docker compose up nonesuch`, which Compose refuses by name in
 * the pane. Missing a marker gives the user nothing and no way to find out why.
 *
 * It is also why nothing here reads `include:`, anchors, or a merge key. Each would need the real
 * resolver, and Compose is the real resolver — it is one process away and it is the thing that
 * will actually run.
 *
 * # The rule
 *
 * Line 1 is always a target, for the whole file. Then: the top-level `services:` key (indent
 * zero), and after it every line whose indent equals the *first* child's and which is a bare
 * `name:` key. The block ends at the next line with indent zero that is neither blank nor a
 * comment — which is how `volumes:` after `services:` stops the scan.
 *
 * Tabs are not indentation in YAML (the spec forbids them), so a line starting with one is left
 * alone rather than counted: a file that uses them is already not a compose file Compose will
 * read, and guessing a width would put markers in the wrong places.
 */
export function composeTargets(text: string): readonly ComposeTarget[] {
  const targets: ComposeTarget[] = [{ line: 1, service: null }]
  const lines = text.split('\n')

  let inServices = false
  let childIndent: number | null = null

  for (let i = 0; i < lines.length; i += 1) {
    const raw = lines[i] ?? ''
    const trimmed = raw.trim()
    if (trimmed === '' || trimmed.startsWith('#')) continue
    if (raw.startsWith('\t')) continue

    const indent = raw.length - raw.trimStart().length

    if (!inServices) {
      if (indent === 0 && /^services:\s*(#.*)?$/.test(trimmed)) inServices = true
      continue
    }

    // Back out to column zero: the `services:` block is over. `volumes:`, `networks:`, or a
    // second `services:` in a malformed file all land here.
    if (indent === 0) {
      inServices = false
      childIndent = null
      // Re-test this very line, because it may itself be another `services:`.
      i -= 1
      continue
    }

    // The first indented line fixes the depth a service name sits at. Everything deeper is a
    // service's *body* — `image:`, `ports:` — and must not be offered as a service.
    childIndent ??= indent
    if (indent !== childIndent) continue

    const match = /^([A-Za-z0-9][A-Za-z0-9._-]*):\s*(#.*)?$/.exec(trimmed)
    if (match?.[1] !== undefined) targets.push({ line: i + 1, service: match[1] })
  }

  return targets
}
