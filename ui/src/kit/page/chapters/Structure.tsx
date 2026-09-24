import { useState, type ReactElement } from 'react'

import { Button, IconButton } from '../../components/Button'
import { Badge, Counter, Dot } from '../../components/Status'
import { Icon } from '@/icons/Icon'
import {
  Breadcrumbs,
  Card,
  CodeBlock,
  Disclosure,
  Heading,
  List,
  ListItem,
  PanelHeader,
  PathGroup,
  PathList,
  PathRow,
  Row,
  Section,
  Summary,
  Table,
  Tabs,
  Toolbar,
  ToolbarSeparator,
  ToolbarSpacer,
} from '../../components/Surface'
import { Cell, Chapter, Frame, Line, Specimen, Stack } from '../Specimen'

export function Structure(): ReactElement {
  const [tab, setTab] = useState<'overview' | 'changes' | 'pipelines'>('changes')
  const [sel, setSel] = useState('Button.tsx')
  return (
    <Chapter
      id="structure"
      title="Panels, cards, lists"
      lead={
        <>
          Surfaces step down one token at a time (<code>--bg</code> → <code>--panel</code> →{' '}
          <code>--panel-2</code> → <code>--chrome-hi</code>); radii grow with the object. One
          rule for rows everywhere: hover is a <code>--panel-2</code> wash, selected is{' '}
          <code>--sel</code>, the open/current one gets a 2px accent bar — the app shows
          &quot;selected&quot; six different ways today.
        </>
      }
    >
      <Specimen
        name="Panel header, section, heading"
        source="Surface.tsx › PanelHeader, Section, Heading"
        use={
          <>
            <strong>PanelHeader</strong> tops every sidebar panel (nine near-copies today), at{' '}
            <code>--h-panelheader</code>. <strong>Section</strong>: an uppercase 11px caption over
            a group — uppercase is used nowhere else, so it always means &quot;a group starts
            here&quot;. <strong>Heading</strong>: a dialog&apos;s or page&apos;s title and one lead
            sentence.
          </>
        }
      >
        <Frame>
          <PanelHeader
            title="Merge requests"
            aside={<Counter value={12} />}
            tools={
              <>
                <IconButton icon="refresh-cw" label="Refresh" />
                <IconButton icon="settings" label="GitLab settings" />
              </>
            }
          />
          <div style={{ padding: 'var(--sp-5)' }}>
            <Section caption="Plan" aside="3 steps">
              <span style={{ fontSize: 'var(--fs-ui-12)', color: 'var(--dim)' }}>Section content.</span>
            </Section>
          </div>
        </Frame>
        <Cell label="heading" grow>
          <Heading
            title="Where should it live?"
            lead="Pick a folder. It is created if it does not exist, and opened in a new project tab."
          />
        </Cell>
      </Specimen>

      <Specimen
        name="Tabs and toolbar"
        source="Surface.tsx › Tabs, Toolbar"
        use={
          <>
            <strong>Tabs</strong> switch <em>what</em> is shown; the current one has the
            horizontal gradient underline. Arrow keys move. (The editor&apos;s tab strip and the
            project tabs are app chrome and keep their own look.) <strong>Toolbar</strong>: icon
            buttons in groups, separated by a hairline.
          </>
        }
        ground="panel"
      >
        <Frame wide>
          <Tabs
            label="Merge request"
            value={tab}
            onChange={setTab}
            tabs={[
              { value: 'overview', label: 'Overview' },
              { value: 'changes', label: 'Changes', aside: <Counter value={14} /> },
              { value: 'pipelines', label: 'Pipelines' },
            ]}
          />
          <Toolbar>
            <IconButton icon="chevrons-down-up" label="Collapse all" />
            <IconButton icon="chevrons-up-down" label="Expand all" />
            <ToolbarSeparator />
            <IconButton icon="columns-2" label="Split view" pressed />
            <IconButton icon="whole-word" label="Ignore whitespace" />
            <ToolbarSpacer />
            <Breadcrumbs trail={['ui', 'src', 'kit', 'components']} />
          </Toolbar>
        </Frame>
      </Specimen>

      <Specimen
        name="Rich list item"
        source="Surface.tsx › List, ListItem"
        use="The GitLab inbox's MR item: small facts, a title (two lines at most), a meta line, a footer of signals. For lists you open things from — MRs, tasks, runs."
        ground="panel"
        flush
        spec={[
          ['padding', '--sp-5 (12)'],
          ['title', '--fs-ui-13, 600, --text-hi, clamps at 2 lines'],
          ['top / meta / foot', '--fs-ui-11, --dim / --dim / --faint'],
          ['hover', '--panel-2'],
          ['current', '2px --accent left bar + accent 5% wash'],
        ]}
      >
        <List label="Merge requests">
          <ListItem
            top={
              <>
                <span style={{ fontFamily: 'var(--font-mono)' }}>!482</span>
                <Badge tone="green" squared>
                  Opened
                </Badge>
              </>
            }
            topAside="2h ago"
            title="Isolated directories per worktree, an honest verify refusal"
            meta="cide / core · feature/isolated-env → master"
            foot={
              <>
                <span>3 threads</span>
                <Badge tone="green" dot>
                  passed
                </Badge>
                <span style={{ color: 'var(--green)' }}>+412</span>
                <span style={{ color: 'var(--red)' }}>−96</span>
              </>
            }
            current
          />
          <ListItem
            top={
              <>
                <span style={{ fontFamily: 'var(--font-mono)' }}>!479</span>
                <Badge tone="purple" squared>
                  Merged
                </Badge>
              </>
            }
            topAside="yesterday"
            title="Milestones and gates, agent MR review, pool limits"
            meta="cide / core · feature/milestones → master"
            foot={
              <Badge tone="red" dot>
                failed
              </Badge>
            }
          />
        </List>
      </Specimen>

      <Specimen
        name="Row and tree row"
        source="Surface.tsx › Row"
        use="One line, 26px: a file, a branch, a setting in a list. Tree rows indent 12px per level so a child's icon sits under its parent's label. Selected is --sel, the same as the file tree."
        ground="panel"
      >
        <Frame>
          <List label="Files" role="tree">
            <Row depth={0} expanded icon="folder" label="components" />
            {['Button.tsx', 'Field.tsx', 'Surface.tsx'].map((f) => (
              <Row
                key={f}
                depth={1}
                icon="file-code"
                label={f}
                detail={f === 'Surface.tsx' ? 'modified' : undefined}
                selected={sel === f}
                onSelect={() => setSel(f)}
                trailing={f === 'Surface.tsx' ? <Badge tone="yellow">M</Badge> : undefined}
              />
            ))}
            <Row depth={0} expanded={false} icon="folder" label="page" />
          </List>
        </Frame>
        <Frame>
          <List label="Branches" role="listbox">
            <Row icon="git-branch" label="master" detail="origin/master" selected />
            <Row icon="git-branch" label="feature/ui-kit" detail="2 ahead" />
            <Row icon="git-branch" label="fix/push-lease" trailing={<Counter value={3} />} />
          </List>
        </Frame>
      </Specimen>

      <Specimen
        name="Card, summary, table"
        source="Surface.tsx › Card, Summary, Table"
        use={
          <>
            <strong>Card</strong>: a box inside a panel or dialog; a toned card is a state (the
            GitLab approval box); a <em>dashed</em> card is proposed, not yet real (a plan, a
            draft comment). <strong>Summary</strong>: key → value, zebra rows.{' '}
            <strong>Table</strong>: several values per row; numbers right-aligned.
          </>
        }
      >
        <Stack>
          <Line>
            <Card title="Approval">Two approvals required; one given.</Card>
            <Card tone="ok">Approved by 2 reviewers.</Card>
            <Card tone="warn">Waiting on 1 more approval.</Card>
            <Card draft>Draft · not posted yet.</Card>
          </Line>
          <Line>
            <Cell label="summary" grow>
              <Summary
                rows={[
                  { label: 'Type', value: 'With subagents' },
                  { label: 'Location', value: '~/work/new-project' },
                  { label: 'Git', value: 'Initialise, branch main' },
                ]}
              />
            </Cell>
            <Cell label="table" grow>
              <Table
                rowKey={(r) => r.job}
                columns={[
                  { key: 'job', label: 'Job', render: (r) => r.job },
                  {
                    key: 'status',
                    label: 'Status',
                    render: (r) => (
                      <Badge tone={r.ok ? 'green' : 'red'} dot>
                        {r.ok ? 'passed' : 'failed'}
                      </Badge>
                    ),
                  },
                  { key: 'time', label: 'Time', numeric: true, render: (r) => r.time },
                ]}
                rows={[
                  { job: 'cargo test', ok: true, time: '4m 12s' },
                  { job: 'clippy', ok: true, time: '1m 03s' },
                  { job: 'ui checks', ok: false, time: '0m 48s' },
                ]}
              />
            </Cell>
          </Line>
        </Stack>
      </Specimen>

      <Specimen
        name="Disclosure and code block"
        source="Surface.tsx › Disclosure, CodeBlock"
        use="A group that folds, and output a user may copy: a log tail, a command, a stack. Mono, selectable, scrolls sideways instead of wrapping."
      >
        <Cell label="disclosure" grow>
          <Disclosure title="Details" aside="12 lines" defaultOpen>
            <CodeBlock>{`$ git push origin master
To github.com:lantian/cide.git
 ! [rejected]        master -> master (fetch first)
error: failed to push some refs`}</CodeBlock>
          </Disclosure>
        </Cell>
      </Specimen>

      <Specimen
        name="Path list"
        source="Surface.tsx › PathList, PathRow, PathGroup"
        use="What an act is about to touch, named one row each — a confirm's files, a close's unsaved tabs, a pull's commits. Sits flush in a Dialog body. Never a count: the name stays whole and the place beside it ellipsises first. The mark carries the tone; the words stay neutral."
        spec={[
          ['row', 'name --fs-ui-13 --text-hi; place mono --fs-ui-11 --faint; 28px side inset'],
          ['height', 'capped at 260 × --ui-scale and scrolls; max={false} leaves it to the dialog'],
          ['group', 'the section caption, 11px 600 uppercase --faint'],
        ]}
      >
        <Cell label="grouped" grow>
          <PathList label="At risk">
            <PathGroup>Unsaved files</PathGroup>
            <PathRow mark={<Dot tone="accent" label="Unsaved" />} name="Overlay.tsx" where="ui/src/kit/components" />
            <PathRow mark={<Dot tone="accent" label="Unsaved" />} name="ui-kit.md" where="docs" />
            <PathGroup>Sessions mid-turn</PathGroup>
            <PathRow mark={<Icon name="square-terminal" size={1} />} name="Claude" where="working" />
            <PathGroup>With answers</PathGroup>
            <PathRow
              mark={<Icon name="swords" size={1} />}
              name="Button.tsx"
              where="ui/src/kit/components"
              trailing={
                <>
                  <Button size="sm">Accept ours</Button>
                  <Button size="sm">Merge…</Button>
                </>
              }
            />
          </PathList>
        </Cell>
      </Specimen>
    </Chapter>
  )
}
