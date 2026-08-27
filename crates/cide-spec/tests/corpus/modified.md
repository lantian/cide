## MODIFIED Requirements

### Requirement: Theme switching
The app SHALL switch themes **per window**, defaulting to the system setting.

#### Scenario: The user picks dark
- **WHEN** the user selects the dark theme in one window
- **THEN** only that window repaints

## REMOVED Requirements

### Requirement: Global theme
**Reason**: superseded by per-window switching.
**Migration**: the last global choice becomes every window's initial choice.
