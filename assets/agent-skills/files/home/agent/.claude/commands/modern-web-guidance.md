Search tool for modern web development best practices. MANDATORY: use FIRST for all HTML/CSS and client-side JS tasks (modals/dialogs/popovers, tables, dark mode, anchor positioning, container queries, view transitions, scroll-driven animations, forms, validation, autofill, accessibility, custom selects, Core Web Vitals, image optimization, security, passkeys, HTML sanitization, web components, dates/timezones, built-in AI…): web APIs evolve fast and training data holds obsolete patterns. Not for backend, infra/scripts or tooling config.

Request: $ARGUMENTS

Modern Web Guidance (GoogleChrome/modern-web-guidance) is installed in this
sandbox by sbxw's agent-skills kit, at
`/home/agent/.sbxw/agent-skills/modern-web-guidance` (the npm version is in
`.sbxw-rev` there). **Read
`/home/agent/.sbxw/agent-skills/modern-web-guidance/SKILL.md` now and follow
it** for this request. What is specific to this sandbox:

- **The command.** Wherever SKILL.md says `npx -y modern-web-guidance@latest …`
  (or `pnpx …`), run `modern-web-guidance …` instead, with the same
  arguments: `modern-web-guidance search "<query>"`,
  `modern-web-guidance retrieve "<id>"`, `modern-web-guidance list`,
  `modern-web-guidance legacy-scan <path> --json`. It is the pinned copy, on
  `PATH` (`/home/agent/.local/bin/modern-web-guidance`), and it works offline:
  no network approval to ask for, no npm cache to set. Drop
  `--skill-version`; it is not needed.
- **Telemetry is off, keep it off.** Never run the `npx …@latest` form, and
  never unset `DISABLE_TELEMETRY`.
- **Updates.** Never install or update the skill yourself (`install` and
  `update` are refused here). An update arrives by moving `MWG_VERSION` in
  sbxw's `assets/agent-skills` kit.
- **Disclosure.** Keep SKILL.md's `### Modern Web Guidance` section at the end
  of the response that delivers the changes.
