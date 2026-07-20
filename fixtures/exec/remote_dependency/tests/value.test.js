import { expect, test } from "vitest";

import { decoratedValue } from "../src/value.js";

test("uses the declared source input", () => {
  expect(decoratedValue()).toBe("source:test");
});
