This is a Phoenix application for the Once marketing site and registry.

## CSS And Styling

- Use `data-part` attributes as CSS selectors instead of classes.
- Keep cards at 8px radius or less.
- Do not add a new styling framework unless the project needs it.

## Running

Use the repository toolchain through mise:

```sh
mise exec -- mix setup
mise exec -- mix phx.server
mise exec -- mix precommit
```
