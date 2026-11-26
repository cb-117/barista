@testable import BaristaCore
import Foundation

/// The same cases as the Rust tests `litellm_prices_convert_to_first_party_entries` and
/// `downloaded_prices_replace_and_extend_the_built_in_table`.
func priceChecks() {
    UsagePricing.use(nil)
    precondition(UsagePricing.price(for: "gpt-6.2-sol")?.model == "gpt-6.2-sol")
    precondition(UsagePricing.price(for: "gpt-5.1-codex")?.model == "gpt-5.1")
    precondition(UsagePricing.checkedOn == ModelPrices.checkedOn)

    let list = """
    {
      "sample_spec": {"litellm_provider": "one of …", "mode": "chat", "input_cost_per_token": 0.0, "output_cost_per_token": 0.0},
      "gpt-6.2-sol": {"litellm_provider": "openai", "mode": "chat", "input_cost_per_token": 2e-6, "output_cost_per_token": 1e-5,
                      "cache_read_input_token_cost": 1e-7, "cache_creation_input_token_cost": 2.5e-6},
      "gpt-5.1-codex": {"litellm_provider": "openai", "mode": "responses", "input_cost_per_token": 1.25e-6, "output_cost_per_token": 1e-5},
      "claude-fable-5-2": {"litellm_provider": "anthropic", "mode": "chat", "input_cost_per_token": 8e-6, "output_cost_per_token": 4e-5,
                           "cache_read_input_token_cost": 2e-7, "cache_creation_input_token_cost": 1e-5},
      "gemini/gemini-4-neon": {"litellm_provider": "gemini", "mode": "chat", "input_cost_per_token": 3e-6, "output_cost_per_token": 1.5e-5},
      "vertex_ai/gemini-4-neon": {"litellm_provider": "vertex_ai", "mode": "chat", "input_cost_per_token": 9e-6, "output_cost_per_token": 9e-5},
      "azure/gpt-6.2-sol": {"litellm_provider": "azure", "mode": "chat", "input_cost_per_token": 9e-6, "output_cost_per_token": 9e-5},
      "text-embedding-4": {"litellm_provider": "openai", "mode": "embedding", "input_cost_per_token": 1e-7, "output_cost_per_token": 1e-7},
      "gpt-free": {"litellm_provider": "openai", "mode": "chat", "input_cost_per_token": 0.0, "output_cost_per_token": 0.0}
    }
    """
    let prices = UsagePricing.convert(litellm: Data(list.utf8))
    precondition(prices.map(\.model) == ["claude-fable-5-2", "gemini-4-neon", "gpt-5.1-codex", "gpt-6.2-sol"], "Converted: \(prices.map(\.model))")
    let sol = prices[3]
    precondition([sol.input, sol.output, sol.cacheRead, sol.cacheWrite5m, sol.cacheWrite1h] == [2, 10, 0.1, 0, 0], "OpenAI bills no cache writes")
    precondition(prices[0].cacheWrite5m == 10 && prices[0].cacheWrite1h == 10, "A missing 1-hour rate is the 5-minute one")
    precondition(prices[1].input == 3, "Gemini's own price, not Vertex's")
    precondition(prices[2].cacheRead == 1.25, "A missing cache-read rate is the input rate")

    UsagePricing.use([
        ModelPrice(model: "gpt-5.1-codex", input: 1, output: 2, cacheRead: 0.1, cacheWrite5m: 0, cacheWrite1h: 0),
        ModelPrice(model: "claude-opus-5-5", input: 3, output: 4, cacheRead: 0.3, cacheWrite5m: 3.75, cacheWrite1h: 6),
    ], fetchedAt: Date(timeIntervalSince1970: 1_795_000_000))
    precondition(UsagePricing.price(for: "gpt-5.1-codex-max")?.model == "gpt-5.1-codex", "A longer downloaded entry wins")
    precondition(UsagePricing.price(for: "claude-opus-5-5[1m]")?.input == 3, "A downloaded entry replaces the built-in one")
    precondition(UsagePricing.price(for: "gpt-5-mini")?.model == "gpt-5-mini", "Built-in models stay")
    precondition(UsagePricing.table.filter { $0.model == "claude-opus-5-5" }.count == 1)
    precondition(UsagePricing.price(for: "codex-auto-review") == nil && UsagePricing.price(for: "codex-auto-review") == nil)
    precondition(UsagePricing.checkedOn == "2026-11-18", "Checked: \(UsagePricing.checkedOn)")
    UsagePricing.use(nil)
    print("Passed price checks: LiteLLM conversion, merging with the built-in table, and the checked date.")
}
