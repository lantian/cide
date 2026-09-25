import { useState, type ReactElement } from 'react'

import { Button, IconButton } from '../../components/Button'
import { Field, FormRow, SearchField, Textarea, TextInput } from '../../components/Field'
import { Select } from '../../components/Select'
import { Kbd } from '../../components/Status'
import { Segmented, Switch } from '../../components/Choice'
import { Cell, Chapter, Line, Specimen, Stack } from '../Specimen'

export function Fields(): ReactElement {
  const [path, setPath] = useState('~/work/new-project')
  const [query, setQuery] = useState('')
  const [branch, setBranch] = useState('main')
  const [format, setFormat] = useState(true)
  const [sort, setSort] = useState('updated')
  const [tab, setTab] = useState('2')
  const [pull, setPull] = useState('ask')
  const [lang, setLang] = useState<string | null>(null)
  return (
    <Chapter
      id="fields"
      title="Fields"
      lead={
        <>
          One box for every kind of text entry: <code>--bg</code> inside the card, so a field
          reads as a hole rather than a raised thing; the accent border plus a 3px halo on focus.
          The app has 22 text-input styles with three different focus treatments today — they all
          become this one.
        </>
      }
    >
      <Specimen
        name="Text input"
        source="Field.tsx › Field, TextInput"
        use={
          <>
            Wrap every input in <code>Field</code>: it wires the label, the hint and the error to
            the control. <strong>mono</strong> for anything a user might paste into a terminal
            (paths, branches, commands). An error says what is wrong in words under the field —
            never a red border alone.
          </>
        }
        ground="chrome"
        spec={[
          ['height', 'md 30px · sm 24px — the same as a button'],
          ['box', '--bg, 1px --border, --r-2; hover border mixes toward --dim'],
          ['focus', '--accent border + 0 0 0 3px accent 20%'],
          ['value / placeholder', '--text-hi / --faint'],
          ['label', '--fs-ui-12, 600, --text; "optional" in --faint 400'],
          ['hint / error', '--fs-ui-11, --faint / --red'],
        ]}
      >
        <Stack>
          <Line>
            <Cell label="default" grow>
              <Field label="Location" hint="The folder is created if it does not exist.">
                {({ id, describedBy }) => (
                  <TextInput
                    id={id}
                    aria-describedby={describedBy}
                    icon="folder"
                    mono
                    value={path}
                    onChange={(e) => setPath(e.target.value)}
                  />
                )}
              </Field>
            </Cell>
            <Cell label="with a button" grow>
              <Field label="Repository" optional>
                {({ id }) => (
                  <Line>
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <TextInput id={id} placeholder="git@host:group/repo.git" mono />
                    </div>
                    <Button>Browse…</Button>
                  </Line>
                )}
              </Field>
            </Cell>
          </Line>
          <Line>
            <Cell label="invalid" grow>
              <Field label="Branch name" error="A branch name cannot contain spaces.">
                {({ id, describedBy, invalid }) => (
                  <TextInput
                    id={id}
                    aria-describedby={describedBy}
                    invalid={invalid}
                    mono
                    defaultValue="feature/ui kit"
                  />
                )}
              </Field>
            </Cell>
            <Cell label="disabled" grow>
              <Field label="Remote">
                {({ id }) => <TextInput id={id} disabled value="origin" readOnly mono />}
              </Field>
            </Cell>
          </Line>
        </Stack>
      </Specimen>

      <Specimen
        name="Search field"
        source="Field.tsx › SearchField"
        use="Filtering a list in a panel. sm in a panel header strip, md in a dialog. A clear button appears once there is a query; a key hint may sit at the right while it is empty."
      >
        <Line>
          <Cell label="sm, empty" grow>
            <SearchField
              size="sm"
              placeholder="Filter merge requests"
              aria-label="Filter merge requests"
              trailing={<Kbd keys={['/']} />}
            />
          </Cell>
          <Cell label="sm, with query" grow>
            <SearchField
              size="sm"
              aria-label="Filter"
              value={query === '' ? 'kit' : query}
              onChange={(e) => setQuery(e.target.value)}
              trailing={<IconButton icon="x" label="Clear" onClick={() => setQuery('')} />}
            />
          </Cell>
          <Cell label="md" grow>
            <SearchField placeholder="Search tasks" aria-label="Search tasks" />
          </Cell>
        </Line>
      </Specimen>

      <Specimen
        name="Select and textarea"
        source="Select.tsx › Select · Field.tsx › Textarea"
        use={
          <>
            <strong>Select</strong> is the kit&apos;s own dropdown, not the OS one: a field-shaped
            trigger and a menu-shaped popup with a check on the chosen option, optional icons and
            a dim detail at the right. Keyboard-complete — arrows, Home/End, Enter, Escape, Tab,
            and a letter jumps to the next option starting with it. The popup opens upward when
            there is no room below and is never clipped by a dialog. <strong>Textarea</strong>{' '}
            for anything longer than a line.
          </>
        }
        ground="chrome"
        spec={[
          ['trigger', 'the field box: md 30 · sm 24, --bg, --r-2, accent focus halo'],
          ['popup', '--panel, 1px --border, --r-3, --shadow-2, max 264px, 4px from the trigger'],
          ['option', '28px, --fs-ui-12; active --sel; chosen check in --accent, label 500'],
          ['detail / disabled', '--fs-ui-11 --faint / --faint, skipped by the keyboard'],
        ]}
      >
        <Line>
          <Cell label="select md" grow>
            <Field label="Base branch">
              {({ id }) => (
                <Select
                  id={id}
                  value={branch}
                  onChange={setBranch}
                  options={[
                    { value: 'main', label: 'main', icon: 'git-branch', detail: 'origin' },
                    { value: 'develop', label: 'develop', icon: 'git-branch', detail: 'origin' },
                    { value: 'release', label: 'release/2026.09', icon: 'git-branch', detail: 'upstream' },
                    { value: 'old', label: 'legacy/1.x', icon: 'git-branch', detail: 'archived', disabled: true },
                  ]}
                />
              )}
            </Field>
          </Cell>
          <Cell label="select sm" grow>
            <Select
              size="sm"
              aria-label="Sort"
              value={sort}
              onChange={setSort}
              options={[
                { value: 'updated', label: 'Recently updated' },
                { value: 'created', label: 'Created' },
                { value: 'title', label: 'Title' },
              ]}
            />
          </Cell>
        </Line>
        <Line>
          <Cell label="placeholder, long list" grow>
            <Select
              aria-label="Language"
              placeholder="Choose a language"
              value={lang}
              onChange={setLang}
              options={[
                'Bash', 'C', 'C++', 'C#', 'Dart', 'Elixir', 'Go', 'Haskell', 'Java', 'JavaScript',
                'Kotlin', 'Lua', 'OCaml', 'PHP', 'Python', 'Ruby', 'Rust', 'Scala', 'Swift',
                'TypeScript', 'Zig',
              ].map((l) => ({ value: l, label: l }))}
            />
          </Cell>
          <Cell label="invalid" grow>
            <Select
              aria-label="Remote"
              placeholder="Choose a remote"
              invalid
              value={null}
              onChange={() => {}}
              options={[{ value: 'origin', label: 'origin' }]}
            />
          </Cell>
          <Cell label="disabled" grow>
            <Select
              aria-label="Provider"
              disabled
              value="gitlab"
              onChange={() => {}}
              options={[{ value: 'gitlab', label: 'GitLab', icon: 'git-merge' }]}
            />
          </Cell>
        </Line>
        <Cell label="textarea" grow>
          <Field label="Instructions" optional hint="Sent to the reviewer with the diff.">
            {({ id, describedBy }) => (
              <Textarea
                id={id}
                aria-describedby={describedBy}
                placeholder="Focus on error handling in the push path…"
              />
            )}
          </Field>
        </Cell>
      </Specimen>

      <Specimen
        name="Settings row"
        source="Field.tsx › FormRow"
        use="A setting: label and one-line hint on the left, the control on the right, a soft rule between rows. A control too wide to share the line wraps under the text."
        ground="panel"
      >
        <div style={{ width: '100%' }}>
          <FormRow label="Format on save" hint="Runs the project formatter before writing the file.">
            <Switch label="Format on save" checked={format} onChange={setFormat} />
          </FormRow>
          <FormRow label="Tab width" hint="Spaces a Tab inserts in files with no editorconfig.">
            <Select
              size="sm"
              aria-label="Tab width"
              value={tab}
              onChange={setTab}
              options={[
                { value: '2', label: '2 spaces' },
                { value: '4', label: '4 spaces' },
              ]}
            />
          </FormRow>
          <FormRow
            label="When your branch has diverged"
            hint="A control too wide to leave the text its floor drops under the text instead of squeezing it into a narrow column."
          >
            <Segmented
              size="sm"
              label="Diverged pull"
              value={pull}
              onChange={setPull}
              options={[
                { value: 'ask', label: 'Ask' },
                { value: 'merge', label: 'Merge' },
                { value: 'rebase', label: 'Rebase' },
                { value: 'ff', label: 'Fast-forward only' },
              ]}
            />
          </FormRow>
        </div>
      </Specimen>
    </Chapter>
  )
}
