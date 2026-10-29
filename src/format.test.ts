import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { formatValue, percentOf } from "./format.ts";

describe("formatValue", () => {
  it("mostra un trattino quando il sensore manca", () => {
    assert.equal(formatValue(null, "°C"), "—");
    assert.equal(formatValue(undefined, "W"), "—");
  });

  it("arrotonda alle cifre richieste", () => {
    assert.equal(formatValue(41.26, "°C", 1), "41.3 °C");
    assert.equal(formatValue(32, "W"), "32 W");
  });
});

describe("percentOf", () => {
  it("restituisce null senza un totale valido", () => {
    assert.equal(percentOf(10, 0), null);
    assert.equal(percentOf(null, 100), null);
  });

  it("limita il risultato tra 0 e 100", () => {
    assert.equal(percentOf(1082, 16304)?.toFixed(1), "6.6");
    assert.equal(percentOf(200, 100), 100);
  });
});
