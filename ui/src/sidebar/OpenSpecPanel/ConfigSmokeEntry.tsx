/**
 * Renders every OpenSpec config story to static HTML and prints one JSON digest line. (M28)
 *
 * Driven by `ui/scripts/check-openspec-config.mjs`. `smokeEntry.tsx` next door is the panel's and
 * is not this change's to edit, so this is its twin for the config surface; the protocol is the
 * same one `check-openspec-render.mjs` expects — the **last** `console.log` is one JSON array.
 * Nothing in the app imports it, so it is tree-shaken out of the real bundle.
 *
 * `configModel.ts` is checked directly under node by the same script. What this adds is the half
 * a pure core cannot speak for:
 *
 *  - that the wizard prints its **context box above its button** in document order. The whole
 *    argument for asking during set-up is that it is the one moment the answer gets written, and
 *    a field below the button is a field that was skipped. Nothing in the model can see that, and
 *    a screenshot cannot defend it against the next tidy-up.
 *  - that a Save in flight leaves **nothing** that can be pressed again. `apply` is an atomic
 *    rename of a committed file and a doubled press is a second one racing the first.
 *  - that the schema `<select>` really contains the value it is showing. A select whose value
 *    matches no option displays a *different* option, silently, and the next Save writes that
 *    other schema over the user's.
 *  - that a class referenced as `styles.x` exists in the stylesheet, which neither `tsc` nor
 *    `vite build` can see.
 */
import { renderToStaticMarkup } from 'react-dom/server'
import { ConfigForm, ConfigWizard } from './ConfigForm'
import {
  CONFIG_STORIES,
  WIZARD_STORIES,
  type ConfigStoryName,
  type WizardStoryName,
} from './configFixture'

/** Every opening tag carrying this hook, void elements included. */
function tags(html: string, hook: string): string[] {
  return [
    ...html.matchAll(new RegExp(`<[a-z][a-z0-9]*[^>]*\\sdata-audit="${hook}"[^>]*>`, 'g')),
  ].map((match) => match[0])
}

function tagOf(html: string, hook: string): string | null {
  return tags(html, hook)[0] ?? null
}

function attr(fragment: string | null, name: string): string | null {
  if (fragment === null) return null
  const match = new RegExp(`${name}="([^"]*)"`).exec(fragment)
  return match === null ? null : (match[1] ?? null)
}

/**
 * Is this tag disabled?
 *
 * `disabled=""` and not `disabled`: `renderToStaticMarkup` writes every boolean attribute with an
 * empty value, so a probe matching only the bare word answers "live" for every control on the
 * screen — which is the shape that would let the saving story pass with every button pressable.
 */
const off = (fragment: string | null): boolean =>
  fragment !== null && /\sdisabled(=|\s|>)/.test(fragment)

const state = (fragment: string | null): 'on' | 'off' | null =>
  fragment === null ? null : off(fragment) ? 'off' : 'on'

/** The text of the whole document with tags and entities gone. */
const text = (html: string): string =>
  html
    .replace(/<[^>]*>/g, ' ')
    .replace(/&#x27;/g, "'")
    .replace(/&quot;/g, '"')
    .replace(/&amp;/g, '&')
    .replace(/\s+/g, ' ')
    .trim()

/** The words inside the status span, which is the only feedback a no-op Save ever gets. */
function statusTextOf(html: string): string | null {
  const match = /<span[^>]*data-audit="specConfigStatus"[^>]*>([\s\S]*?)<\/span>/.exec(html)
  return match === null ? null : text(match[1] ?? '')
}

export interface ConfigDigest {
  story: string
  /** The root rendered at all. */
  root: 'form' | 'wizard' | null
  /** Everything that writes, by its hook — `<input|textarea|select|button data-write>`. */
  writeControls: string[]
  /** Of those, the ones that are still live. */
  liveWrites: string[]
  /** The schema select's value, and the options it offers. */
  schemaValue: string | null
  schemaOptions: string[]
  schemaHint: string | null
  /**
   * Is the "OpenSpec ships one workflow" sentence drawn? (M28)
   *
   * A picker with one option reads as a broken control or a hard-coded value; it is neither, and
   * nothing on screen said so until somebody asked.
   */
  schemaOnlyOne: boolean
  schemaUnlisted: string | null
  /** Whether the file's own path is on screen, and openable. */
  path: string | null
  openFile: 'on' | 'off' | null
  /** The context box: whether it exists, and what it holds. */
  context: boolean
  contextRows: string | null
  /** The rules and guidance lists, as `<group>|<rows>`. */
  ruleLists: string[]
  guidanceLists: string[]
  /** Save and Discard. */
  save: 'on' | 'off' | null
  revert: 'on' | 'off' | null
  status: string | null
  statusText: string | null
  error: string | null
  cliNote: string | null
  /** The wizard's button, its promised action, and whether the box comes first. */
  setUp: 'on' | 'off' | null
  setUpAction: string | null
  contextBeforeSetUp: boolean | null
  wizardPath: string | null
  /** Elements carrying a hook but no class, plus any class list holding `undefined`. */
  unclassed: number
  /** Does the whole document say the words that make `context` worth filling in? */
  saysEveryPrompt: boolean
}

function digest(story: string, root: 'form' | 'wizard', html: string): ConfigDigest {
  const writes = [...html.matchAll(/<[a-z][a-z0-9]*[^>]*\sdata-write="true"[^>]*>/g)].map(
    (match) => match[0],
  )
  const hookOf = (tag: string): string => attr(tag, 'data-audit') ?? '?'

  const hooked = [...html.matchAll(/<[a-z][a-z0-9]*[^>]*\sdata-audit="[^"]*"[^>]*>/g)]
  const unclassed = hooked.filter((match) => {
    const cls = /class="([^"]*)"/.exec(match[0])
    return cls === null || cls[1] === undefined || cls[1].includes('undefined')
  }).length

  const contextAt = html.indexOf('data-audit="specConfigWizardContext"')
  const setUpAt = html.indexOf('data-audit="specConfigSetUp"')
  const setUpTag = tagOf(html, 'specConfigSetUp')

  const listRows = (hook: string): string[] =>
    tags(html, `${hook}List`).map((tag) => {
      const group = attr(tag, 'data-group') ?? '?'
      const rows = tags(html, `${hook}Item`).filter(
        (item) => attr(item, 'data-group') === group,
      ).length
      return `${group}|${rows}`
    })

  const selectBody = /<select[^>]*data-audit="specConfigSchema"[^>]*>([\s\S]*?)<\/select>/.exec(html)

  /*
   * The `<select>`'s value is **not** an attribute on the select.
   *
   * React renders a controlled select by marking the matching `<option selected="">`, and puts
   * nothing on the select itself. A digest that read `value=` off the select would answer `null`
   * for every story — and `null` would then compare equal to `null` in the one assertion that
   * matters here, which is that the value the form is showing is among the options it offers.
   */
  const optionsBody = selectBody === null ? '' : (selectBody[1] ?? '')
  const optionTags = [...optionsBody.matchAll(/<option[^>]*>/g)].map((match) => match[0])
  const selected = optionTags.find((tag) => /\sselected(=|\s|>)/.test(tag)) ?? null

  const errorShown =
    tagOf(html, 'specConfigError') !== null || tagOf(html, 'specConfigWizardError') !== null

  return {
    story,
    root,
    writeControls: writes.map(hookOf),
    liveWrites: writes.filter((tag) => !off(tag)).map(hookOf),
    schemaValue: attr(selected, 'value'),
    schemaOptions: optionTags.map((tag) => attr(tag, 'value') ?? ''),
    schemaHint: tagOf(html, 'specConfigSchemaHint') === null ? null : 'shown',
    schemaOnlyOne: tagOf(html, 'specConfigSchemaOnlyOne') !== null,
    schemaUnlisted: attr(tagOf(html, 'specConfigSchemaHint'), 'data-unlisted'),
    path: tagOf(html, 'specConfigPath') === null ? null : 'shown',
    openFile: state(tagOf(html, 'specConfigOpenFile')),
    context: tagOf(html, 'specConfigContext') !== null,
    contextRows: attr(tagOf(html, 'specConfigContext'), 'rows'),
    ruleLists: listRows('specConfigRule'),
    guidanceLists: listRows('specConfigGuidance'),
    save: state(tagOf(html, 'specConfigSave')),
    revert: state(tagOf(html, 'specConfigRevert')),
    status: attr(tagOf(html, 'specConfigStatus'), 'data-status'),
    statusText: statusTextOf(html),
    error: errorShown ? 'shown' : null,
    cliNote: tagOf(html, 'specConfigCliNote') === null ? null : 'shown',
    setUp: state(setUpTag),
    setUpAction: attr(setUpTag, 'data-action'),
    contextBeforeSetUp: setUpAt < 0 ? null : contextAt >= 0 && contextAt < setUpAt,
    wizardPath: tagOf(html, 'specConfigWizardPath') === null ? null : 'shown',
    unclassed,
    // The sentence that makes the field worth filling in. It is the one piece of prose on this
    // screen that is load-bearing rather than decorative: without it `context` is an unexplained
    // box, and an unexplained box is left empty.
    saysEveryPrompt: /every artifact-generation prompt/i.test(text(html)),
  }
}

const digests: ConfigDigest[] = [
  ...(Object.keys(CONFIG_STORIES) as ConfigStoryName[]).map((story) =>
    digest(story, 'form', renderToStaticMarkup(<ConfigForm {...CONFIG_STORIES[story]} />)),
  ),
  ...(Object.keys(WIZARD_STORIES) as WizardStoryName[]).map((story) =>
    digest(
      `wizard-${story}`,
      'wizard',
      renderToStaticMarkup(<ConfigWizard {...WIZARD_STORIES[story]} />),
    ),
  ),
]

console.log(JSON.stringify(digests))
