import { useState, type ReactElement } from 'react'

import { Icon } from '@/icons/Icon'
import { Button, IconButton } from '../../components/Button'
import { Segmented, Switch } from '../../components/Choice'
import { Banner } from '../../components/Feedback'
import { FormRow, SearchField } from '../../components/Field'
import { Select } from '../../components/Select'
import { Badge, Counter, Kbd } from '../../components/Status'
import { List, ListItem, PanelHeader, Section } from '../../components/Surface'
import { Chapter, Frame, Specimen } from '../Specimen'
import styles from '../kit.module.css'

const RULES: ReadonlyArray<{ kind: 'do' | 'dont'; title: string; text: string }> = [
  { kind: 'do', title: 'One primary per surface', text: 'A footer, a row, an empty state: one gradient button, the act the surface exists for.' },
  { kind: 'dont', title: 'A second accent', text: 'Filled accent chips, accent-filled rows, orange headings. The accent marks the current thing and the primary act only.' },
  { kind: 'do', title: 'Tokens, always', text: 'Every colour, size, radius, gap and duration is a token. A number that is not a token is a token to add, first, in tokens.css.' },
  { kind: 'dont', title: 'Literal pixels on text', text: 'A size around text is calc(Npx * var(--ui-scale)), literal first; font sizes are --fs-ui-* rungs only.' },
  { kind: 'do', title: 'Same height, same row', text: 'Buttons and fields in one row share md (30) or sm (24). Icon buttons are square at the same height.' },
  { kind: 'dont', title: 'Colour alone', text: 'A red border without words, a dot without a title, a state without its badge text.' },
  { kind: 'do', title: 'Selected = --sel, current = accent bar', text: 'Hover --panel-2; selected --sel; the open/current item a 2px accent bar and a 5% wash.' },
  { kind: 'dont', title: 'Unicode for icons', text: 'A multiplication sign, a check mark or a triangle typed as text is not an icon. Use <Icon>; a missing mark is vendored, not typed.' },
]

export function Patterns(): ReactElement {
  const [scope, setScope] = useState<'assigned' | 'reviewing' | 'created'>('assigned')
  const [autosave, setAutosave] = useState(true)
  const [fontSize, setFontSize] = useState('12.5')
  return (
    <Chapter
      id="patterns"
      title="Patterns"
      lead="Whole surfaces put together from the parts above — what a new sidebar panel or settings page should look like end to end — and the rules the parts follow."
    >
      <Specimen
        name="A sidebar panel"
        source="composed"
        use="The GitLab inbox, rebuilt from kit parts: a panel header with tools, a filter strip (segmented scope + search), a caption with the count, rich list items, a footer line."
        ground="bg"
      >
        <Frame>
          <PanelHeader
            title="Merge requests"
            tools={
              <>
                <IconButton icon="refresh-cw" label="Refresh" />
                <IconButton icon="settings" label="GitLab settings" />
              </>
            }
          />
          <div
            style={{
              display: 'flex',
              flexDirection: 'column',
              gap: 'var(--sp-4)',
              padding: 'var(--sp-5)',
              borderBottom: '1px solid var(--border-soft)',
            }}
          >
            <Segmented
              label="Scope"
              block
              value={scope}
              onChange={setScope}
              options={[
                { value: 'assigned', label: 'Assigned' },
                { value: 'reviewing', label: 'Reviewing' },
                { value: 'created', label: 'Created' },
              ]}
            />
            <SearchField size="sm" placeholder="Filter" aria-label="Filter" trailing={<Kbd keys={['/']} />} />
          </div>
          <Banner action={<Button variant="link">Refresh</Button>}>2 new since you looked.</Banner>
          <div style={{ padding: 'var(--sp-5) var(--sp-5) 0' }}>
            <Section caption="Open" aside={<Counter value={2} />}>
              <span />
            </Section>
          </div>
          <List label="Merge requests">
            <ListItem
              current
              top={
                <>
                  <span style={{ fontFamily: 'var(--font-mono)' }}>!482</span>
                  <Badge tone="green" squared>
                    Opened
                  </Badge>
                </>
              }
              topAside="2h"
              title="Isolated directories per worktree"
              meta="feature/isolated-env → master"
              foot={
                <>
                  <span>
                    <Icon name="message-square" size={0} /> 3
                  </span>
                  <Badge tone="green" dot>
                    passed
                  </Badge>
                </>
              }
            />
            <ListItem
              top={<span style={{ fontFamily: 'var(--font-mono)' }}>!481</span>}
              topAside="5h"
              title="The phone: pair a device and watch runs"
              meta="feature/remote → master"
              foot={
                <Badge tone="yellow" dot>
                  pending
                </Badge>
              }
            />
          </List>
        </Frame>
      </Specimen>

      <Specimen
        name="A settings section"
        source="composed"
        use="A settings page: the page title, a section caption per group, form rows, and a primary only when the section needs an explicit Save (most settings apply at once)."
        ground="panel"
      >
        <div style={{ width: '100%', maxWidth: 'calc(560px * var(--ui-scale))' }}>
          <Section caption="Editor">
            <FormRow label="Save automatically" hint="When the window loses focus.">
              <Switch label="Save automatically" checked={autosave} onChange={setAutosave} />
            </FormRow>
            <FormRow label="Font size" hint="For code; the interface size is under Appearance.">
              <Select
                size="sm"
                aria-label="Font size"
                value={fontSize}
                onChange={setFontSize}
                options={[
                  { value: '12.5', label: '12.5 px' },
                  { value: '14', label: '14 px' },
                ]}
              />
            </FormRow>
          </Section>
        </div>
      </Specimen>

      <Specimen name="Rules" source="docs/ui-kit.md" use="The short version of the rules the kit is built on.">
        <div className={styles.dosDonts}>
          {RULES.map((r) => (
            <div key={r.title} className={styles.rule} data-kind={r.kind}>
              <div className={styles.ruleTitle}>
                <Icon name={r.kind === 'do' ? 'circle-check' : 'circle-x'} size={1} />
                {r.title}
              </div>
              {r.text}
            </div>
          ))}
        </div>
      </Specimen>
    </Chapter>
  )
}
