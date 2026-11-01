import Foundation

/// API list prices used for usage cost *estimates*, in US dollars per million tokens.
/// These are pay-as-you-go API rates, not what a Claude, ChatGPT, or Cursor subscription charges.
///
/// This JSON is the one price table for every platform: the Rust build (rust/barista/build.rs)
/// extracts the text between the `#"""` markers below. Keep it valid JSON.
///
/// Matching: a log's model ID is cut at the first `[` or `@` (e.g. "claude-opus-5-5[1m]"), then the
/// longest `model` that equals it, or is followed in it by "-", wins. So "claude-opus-4-1-20250805"
/// prices as "claude-opus-4-1" and "gpt-5.1-codex" as "gpt-5.1". Unlisted models show tokens with no cost.
///
/// Sources (checked 2026-11-01): Anthropic's model pricing (cache writes are 1.25× input for 5-minute
/// entries and 2× for 1-hour entries; cache reads are priced per model), and
/// https://developers.openai.com/api/docs/pricing (Standard tier, short-context rates; OpenAI bills
/// no cache writes, and models without a cached rate bill cached input at the full input price).
public enum ModelPrices {
    public static let checkedOn = "2026-11-01"

    public static let json = #"""
    [
      {"model": "claude-fable-5-1",  "input": 10,   "output": 50,   "cacheRead": 0.25, "cacheWrite5m": 12.5,  "cacheWrite1h": 20},
      {"model": "claude-mythos-5-1", "input": 10,   "output": 50,   "cacheRead": 0.25, "cacheWrite5m": 12.5,  "cacheWrite1h": 20},
      {"model": "claude-fable-5",    "input": 10,   "output": 50,   "cacheRead": 1,    "cacheWrite5m": 12.5,  "cacheWrite1h": 20},
      {"model": "claude-mythos-5",   "input": 10,   "output": 50,   "cacheRead": 1,    "cacheWrite5m": 12.5,  "cacheWrite1h": 20},
      {"model": "claude-opus-5-5",   "input": 4,    "output": 20,   "cacheRead": 0.2,  "cacheWrite5m": 5,     "cacheWrite1h": 8},
      {"model": "claude-opus-5",     "input": 5,    "output": 25,   "cacheRead": 0.5,  "cacheWrite5m": 6.25,  "cacheWrite1h": 10},
      {"model": "claude-opus-4-8",   "input": 5,    "output": 25,   "cacheRead": 0.5,  "cacheWrite5m": 6.25,  "cacheWrite1h": 10},
      {"model": "claude-opus-4-7",   "input": 5,    "output": 25,   "cacheRead": 0.5,  "cacheWrite5m": 6.25,  "cacheWrite1h": 10},
      {"model": "claude-opus-4-6",   "input": 5,    "output": 25,   "cacheRead": 0.5,  "cacheWrite5m": 6.25,  "cacheWrite1h": 10},
      {"model": "claude-opus-4-5",   "input": 5,    "output": 25,   "cacheRead": 0.5,  "cacheWrite5m": 6.25,  "cacheWrite1h": 10},
      {"model": "claude-opus-4-1",   "input": 15,   "output": 75,   "cacheRead": 1.5,  "cacheWrite5m": 18.75, "cacheWrite1h": 30},
      {"model": "claude-opus-4",     "input": 15,   "output": 75,   "cacheRead": 1.5,  "cacheWrite5m": 18.75, "cacheWrite1h": 30},
      {"model": "claude-sonnet-5-5", "input": 2,    "output": 10,   "cacheRead": 0.2,  "cacheWrite5m": 2.5,   "cacheWrite1h": 4},
      {"model": "claude-sonnet-5",   "input": 2,    "output": 10,   "cacheRead": 0.2,  "cacheWrite5m": 2.5,   "cacheWrite1h": 4},
      {"model": "claude-sonnet-4-6", "input": 3,    "output": 15,   "cacheRead": 0.3,  "cacheWrite5m": 3.75,  "cacheWrite1h": 6},
      {"model": "claude-sonnet-4-5", "input": 3,    "output": 15,   "cacheRead": 0.3,  "cacheWrite5m": 3.75,  "cacheWrite1h": 6},
      {"model": "claude-sonnet-4",   "input": 3,    "output": 15,   "cacheRead": 0.3,  "cacheWrite5m": 3.75,  "cacheWrite1h": 6},
      {"model": "claude-haiku-5-5",  "input": 0.1,  "output": 0.5,  "cacheRead": 0.01, "cacheWrite5m": 0.125, "cacheWrite1h": 0.2},
      {"model": "claude-haiku-4-5",  "input": 1,    "output": 5,    "cacheRead": 0.1,  "cacheWrite5m": 1.25,  "cacheWrite1h": 2},
      {"model": "gpt-6-astra",       "input": 10,   "output": 50,   "cacheRead": 1,     "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-6.1-sol",       "input": 2,    "output": 10,   "cacheRead": 0.1,   "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-6.1-terra",     "input": 1,    "output": 5,    "cacheRead": 0.05,  "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-6-sol",         "input": 2,    "output": 10,   "cacheRead": 0.2,   "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-6-luna",        "input": 0.1,  "output": 0.5,  "cacheRead": 0.01,  "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-5.6-sol",       "input": 4,    "output": 20,   "cacheRead": 0.4,   "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-5.6-terra",     "input": 2,    "output": 12,   "cacheRead": 0.2,   "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-5.6-luna",      "input": 0.2,  "output": 1.2,  "cacheRead": 0.02,  "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-5.5",           "input": 5,    "output": 30,   "cacheRead": 0.5,   "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-5.5-pro",       "input": 30,   "output": 180,  "cacheRead": 30,    "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-5.4",           "input": 2.5,  "output": 15,   "cacheRead": 0.25,  "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-5.4-pro",       "input": 30,   "output": 180,  "cacheRead": 30,    "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-5.4-mini",      "input": 0.75, "output": 4.5,  "cacheRead": 0.075, "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-5.4-nano",      "input": 0.2,  "output": 1.25, "cacheRead": 0.02,  "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-5.3-codex",     "input": 1.75, "output": 14,   "cacheRead": 0.175, "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-5.2",           "input": 1.75, "output": 14,   "cacheRead": 0.175, "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-5.2-pro",       "input": 21,   "output": 168,  "cacheRead": 21,    "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-5.1",           "input": 1.25, "output": 10,   "cacheRead": 0.125, "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-5",             "input": 1.25, "output": 10,   "cacheRead": 0.125, "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-5-mini",        "input": 0.25, "output": 2,    "cacheRead": 0.025, "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-5-nano",        "input": 0.05, "output": 0.4,  "cacheRead": 0.005, "cacheWrite5m": 0, "cacheWrite1h": 0},
      {"model": "gpt-5-pro",         "input": 15,   "output": 120,  "cacheRead": 15,    "cacheWrite5m": 0, "cacheWrite1h": 0}
    ]
    """#
}
