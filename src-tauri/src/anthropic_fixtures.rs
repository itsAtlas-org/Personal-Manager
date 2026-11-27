// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Anthropic's real replies to Bobby's key, 2026-11-26 (anthropic-version 2026-10-21), copied verbatim
//! from the live probes (`anthropic-probe2.txt`, `anthropic-replies.txt`). No key is in them; the one
//! long signature is Anthropic's own, as the probe saved it. `anthropic_direct`'s tests read them;
//! nothing else does.
//!
//! The streams carry whitespace padding INSIDE the JSON before the last brace, and `message_start`
//! carries a placeholder `output_tokens`: both are exactly as Anthropic sent them.

/// Probe 2 (e): a plain streamed reply on Haiku 5.5 ("Count from 1 to 5"). The final usage is in
/// `message_delta`, never a sum with `message_start`.
pub(super) const STREAM_HAIKU: &str = r####"event: message_start
data: {"type":"message_start","message":{"model":"claude-haiku-5-5","id":"msg_011ChRstJrw9PYH9sDphBwY4","type":"message","role":"assistant","content":[],"container":null,"stop_reason":null,"stop_sequence":null,"stop_details":null,"usage":{"input_tokens":23,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0},"output_tokens":8,"service_tier":"standard","inference_geo":"global","speed":"standard"},"diagnostics":null}         }

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}             }

event: ping
data: {"type": "ping"}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"1\n2\n3\n4"}   }

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"\n5"}     }

event: content_block_stop
data: {"type":"content_block_stop","index":0        }

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null,"stop_details":null,"container":null},"usage":{"input_tokens":23,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":11,"output_tokens_details":{"thinking_tokens":0}}          }

event: message_stop
data: {"type":"message_stop"   }
"####;

/// Probe 2 (f): a streamed reply on Sonnet 5.5 at effort max, with a thinking block that arrives EMPTY
/// (one empty `thinking_delta`, then a `signature_delta`) before the text block.
pub(super) const STREAM_SONNET_THINKING: &str = r####"event: message_start
data: {"type":"message_start","message":{"model":"claude-sonnet-5-5","id":"msg_011ChRstNbgDKTJc7wApxLaL","type":"message","role":"assistant","content":[],"container":null,"stop_reason":null,"stop_sequence":null,"stop_details":null,"usage":{"input_tokens":45,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0},"output_tokens":8,"service_tier":"standard","inference_geo":"global","speed":"standard"},"diagnostics":null}            }

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}       }

event: ping
data: {"type": "ping"}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":""}        }

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"CAQSgwUKEAgUGAI4AUIIdGhpbmtpbmcSDIjt+/[long-token]//[long-token]/qw9pxv5iWmmvUJ2d/aaoIzC7IwunFJE9ZZ+lXI+rSwo5C9SZOMdMI4GnrKz7NO21gYD+wMQdgbowqaJBmDLAK3XcN+Aga3exKdYNKXRa+PrJ88uUczhFNeHKLmn9ua7Xbw2Jwi/xBrD9cOdayShpPo1/tr03smg8k7kgBOcDu70+e7Ef4xtyTS/vX9kknXR8dVlFcPeAcrksGrgVAlU+eSQ+[long-token]/[long-token]+4NUm7U3Rsysn5mSXQwSQFU2UP/bRmXS1iJoP29ZY0+[long-token]+[long-token]/[long-token]/MARHxTjhs9SET+aGAE="}         }

event: content_block_stop
data: {"type":"content_block_stop","index":0               }

event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}             }

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"£"}  }

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"18"}  }

event: content_block_stop
data: {"type":"content_block_stop","index":1     }

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null,"stop_details":null,"container":null},"usage":{"input_tokens":45,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":36,"output_tokens_details":{"thinking_tokens":32}}    }

event: message_stop
data: {"type":"message_stop"              }
"####;

/// A non-streamed 200 (Haiku 5.5, thinking disabled, effort low): the shape of the key check's test chat.
pub(super) const HAIKU_LOW_200: &str = r####"{"model":"claude-haiku-5-5","id":"msg_011ChRssjxGnKqXT2WdWSTyc","type":"message","role":"assistant","content":[{"type":"text","text":"OK"}],"container":null,"stop_reason":"end_turn","stop_sequence":null,"stop_details":null,"usage":{"input_tokens":13,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0},"output_tokens":4,"output_tokens_details":{"thinking_tokens":0},"service_tier":"standard","inference_geo":"global","speed":"standard"},"diagnostics":null}"####;

/// 400 for `xhigh` with thinking disabled on Haiku (probe 2 (a)).
pub(super) const XHIGH_400: &str = r####"{"type":"error","error":{"type":"invalid_request_error","message":"output_config.effort 'xhigh' is not supported when thinking is disabled on this model. Use effort 'high' or below, or enable thinking."},"request_id":"req_011ChRsssePLoEFj8AaE9p7W"}"####;

/// 400 for a blank user message (probe 2 (h)).
pub(super) const BLANK_400: &str = r####"{"type":"error","error":{"type":"invalid_request_error","message":"messages: text content blocks must contain non-whitespace text"},"request_id":"req_011ChRstXm5eNUzns4KoUz4y"}"####;

/// 400 for `temperature` (probe 2 (i)).
pub(super) const TEMPERATURE_400: &str = r####"{"type":"error","error":{"type":"invalid_request_error","message":"temperature: Extra inputs are not permitted"},"request_id":"req_011ChRsta3zRrZMfXcWAVEyf"}"####;

/// 200 that stopped at `max_tokens` 1 (probe 2 (g)).
pub(super) const MAX_TOKENS_200: &str = r####"{"model":"claude-haiku-5-5","id":"msg_011ChRstV3MuCr8owx3e7xdh","type":"message","role":"assistant","content":[{"type":"text","text":"O"}],"container":null,"stop_reason":"max_tokens","stop_sequence":null,"stop_details":null,"usage":{"input_tokens":15,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0},"output_tokens":1,"output_tokens_details":{"thinking_tokens":0},"service_tier":"standard","inference_geo":"global","speed":"standard"},"diagnostics":null}"####;

/// One entry of Bobby's real `GET /v1/models` listing (`claude-fable-5-2`), verbatim.
pub(super) const MODEL_ENTRY_FABLE_5_2: &str = r####"{"type":"model","id":"claude-fable-5-2","display_name":"Claude Fable 5.2","created_at":"2026-11-09T18:30:00Z","line":"fable","max_input_tokens":1000000,"max_tokens":128000,"capabilities":{"batch":{"supported":true},"citations":{"supported":true},"code_execution":{"supported":true},"context_management":{"supported":true,"clear_tool_uses_20250919":{"supported":true},"clear_thinking_20251015":{"supported":true},"compact_20260112":{"supported":true}},"effort":{"supported":true,"low":{"supported":true},"medium":{"supported":true},"high":{"supported":true},"xhigh":{"supported":true},"max":{"supported":true}},"image_input":{"supported":true},"pdf_input":{"supported":true},"server_tools":{"supported":true,"code_execution":{"supported":true},"web_search":{"supported":true}},"structured_outputs":{"supported":true},"thinking":{"supported":true,"types":{"enabled":{"supported":false},"adaptive":{"supported":true},"disabled":{"supported":false},"between_tools":{"supported":false}}}},"lifecycle":"active","deprecated_at":null,"retires_at":null}"####;

/// One entry of Bobby's real `GET /v1/models` listing (`claude-haiku-5-5`), verbatim.
pub(super) const MODEL_ENTRY_HAIKU_5_5: &str = r####"{"type":"model","id":"claude-haiku-5-5","display_name":"Claude Haiku 5.5","created_at":"2026-10-07T18:00:00Z","line":"haiku","max_input_tokens":1000000,"max_tokens":128000,"capabilities":{"batch":{"supported":true},"citations":{"supported":true},"code_execution":{"supported":true},"context_management":{"supported":true,"clear_tool_uses_20250919":{"supported":true},"clear_thinking_20251015":{"supported":true},"compact_20260112":{"supported":true}},"effort":{"supported":true,"low":{"supported":true},"medium":{"supported":true},"high":{"supported":true},"xhigh":{"supported":true},"max":{"supported":true}},"image_input":{"supported":true},"pdf_input":{"supported":true},"server_tools":{"supported":true,"code_execution":{"supported":true},"web_search":{"supported":true}},"structured_outputs":{"supported":true},"thinking":{"supported":true,"types":{"enabled":{"supported":false},"adaptive":{"supported":true},"disabled":{"supported":true},"between_tools":{"supported":false}}}},"lifecycle":"active","deprecated_at":null,"retires_at":null}"####;
