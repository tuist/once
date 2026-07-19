import { expect, test } from "vitest"

test("converts text to uppercase", () => {
  expect("once".toUpperCase()).toBe("ONCE")
})
