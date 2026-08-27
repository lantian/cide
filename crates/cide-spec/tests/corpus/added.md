# Dark mode

## ADDED Requirements

### Requirement: Theme switching
The app SHALL switch between a light and a dark theme, defaulting to the system setting.

#### Scenario: The user picks dark
- **WHEN** the user selects the dark theme
- **THEN** every open editor repaints without a relaunch

#### Scenario: The system decides
- **WHEN** no explicit theme has been chosen
- **THEN** the app follows the operating system

### Requirement: Remembering the choice
The app SHALL remember an explicit choice across restarts.

#### Scenario: Relaunch
- **WHEN** the app is restarted
- **THEN** the theme is the one that was chosen
