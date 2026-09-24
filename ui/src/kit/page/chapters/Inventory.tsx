/**
 * What is left of the app's own drawing after the redesign (2026-09-24), kind by kind. The first
 * version of this table counted the separately styled copies of each kind *before* the redesign
 * (~45 secondary buttons, ~35 icon buttons, 6 drawings of "selected", …) to size the work; this
 * one lists what did not become a kit component, and why, so the next change knows which of the
 * remaining surfaces is a debt and which is a decision. When a row is paid off, delete it.
 */
import type { ReactElement } from 'react'

import { Chapter, Specimen } from '../Specimen'
import styles from '../kit.module.css'

const ROWS: ReadonlyArray<readonly [string, string, string]> = [
  ['Buttons in pinned markup', 'composes the kit class', 'Agents settings, task card, panel rows: check scripts read the elements, so they keep them and compose Button.module.css'],
  ['Fields with a state', 'kit field numbers on a bare <input>', 'a struck-through refused CLI argument, a column-sized pool cell, the task card (its render check counts native controls)'],
  ['Native <select> in the task card', 'kit field numbers', 'check:agents-render counts the options; the chevron is still the platform arrow'],
  ['Chrome markup', 'composes Chrome.module.css', 'tab strip, rail, status bar, pane bar keep their elements — drag, overflow, detach and the layout audit read them'],
  ['Traffic lights', 'kit tone gradients', 'their 11px size is the layout audit\'s; not WindowControls'],
  ['TriCheckbox', 'kept', 'a tri-state box in a tree; the kit Checkbox has no partial row state of its own'],
  ['Tooltip', 'native title', 'about 200 uses; the kit Tooltip is drawn but not wired to anything yet'],
  ['Avatar / Person', 'partly used', 'the GitLab panel\'s commit authors and draft discussions use Person; the task log and GitLab threads are still plain text'],
  ['File-type badges', 'own', 'the picker\'s `TS`/`RS` column is a coloured mono label, not a status'],
  ['Renderers', 'tokens only', 'diff, merge, commit graph, terminal, the editor surface and splitter drag geometry'],
]

export function Inventory(): ReactElement {
  return (
    <Chapter
      id="inventory"
      title="What the app has today"
      lead="The redesign moved the app onto the kit (2026-09-24). This is what did not become a kit component, how it is drawn instead, and why."
    >
      <Specimen
        name="Inventory"
        source="after the redesign, 2026-09-24"
        use="A row is either a decision (the reason says why the kit part does not fit) or a debt (the reason says what is in the way). Delete a row when it is paid off."
        ground="panel"
      >
        <table className={styles.inventory}>
          <thead>
            <tr>
              <th>Kind</th>
              <th>Drawn as</th>
              <th>Why</th>
            </tr>
          </thead>
          <tbody>
            {ROWS.map(([kind, how, why]) => (
              <tr key={kind}>
                <td>{kind}</td>
                <td>
                  <code>{how}</code>
                </td>
                <td>{why}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </Specimen>
    </Chapter>
  )
}
