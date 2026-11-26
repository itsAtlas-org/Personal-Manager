// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Google's real replies to Bobby's two keys (a free one and a paid one), 2026-11-26, copied
//! verbatim (no key is in them; the long base64 strings are Google's own `thought_signature`
//! values). One exception: the Neon probe's `continuationToken` is blanked, because PM never reads
//! it and the repo's secret scan reads an opaque token beside that name as a leaked key. The first block is the tier spike's; the block from `PAID_PROBE_200_PRO` on is the
//! build-day run (`build-day-paid.txt` and `build-day-free.txt`), each constant named for its step.
//! `gemini_direct`'s tests read them; nothing else does. The 4 KiB native list in the spike is cut
//! off mid-model, so the list fixtures are hand-trimmed in the tests instead.
//!
//! One caution from the build-day run: its free step 3 (the native `generateContent` on 3.1
//! Flash-Lite, for a free-versus-paid diff) timed out with no bytes, and the text under that heading
//! in the raw file is a stale copy of the step before it, so nothing here is taken from it.

/// The free key on a model only paid keys can use, native endpoint (HTTP 429).
pub(crate) const FREE_429_NATIVE: &str = r####"{
  "error": {
    "code": 429,
    "message": "You exceeded your current quota, please check your plan and billing details. For more information on this error, head to: https://aistudio.google.com/docs/rate-limits. To monitor your current usage, head to: https://ai.dev/rate-limit. \n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_requests, limit: 0, model: gemini-3.1-pro, description: Generate content requests per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_requests, limit: 0, model: gemini-3.1-pro, description: Generate content requests per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_input_token_count, limit: 0, model: gemini-3.1-pro, description: Generate content input tokens per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_input_token_count, limit: 0, model: gemini-3.1-pro, description: Generate content input tokens per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\nPlease retry in 19h38m15.023373064s.",
    "status": "RESOURCE_EXHAUSTED",
    "details": [
      {
        "@type": "type.googleapis.com/google.rpc.Help",
        "links": [
          {
            "description": "Learn more about Gemini API quotas",
            "url": "https://aistudio.google.com/docs/rate-limits"
          }
        ]
      },
      {
        "@type": "type.googleapis.com/google.rpc.QuotaFailure",
        "violations": [
          {
            "description": "Generate content requests per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.",
            "quotaMetric": "generativelanguage.googleapis.com/generate_content_free_tier_requests",
            "quotaId": "GenerateRequestsPerDayPerProjectPerModel-FreeTier",
            "quotaDimensions": {
              "model": "gemini-3.1-pro",
              "location": "global"
            }
          },
          {
            "description": "Generate content requests per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.",
            "quotaMetric": "generativelanguage.googleapis.com/generate_content_free_tier_requests",
            "quotaId": "GenerateRequestsPerMinutePerProjectPerModel-FreeTier",
            "quotaDimensions": {
              "model": "gemini-3.1-pro",
              "location": "global"
            }
          },
          {
            "description": "Generate content input tokens per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.",
            "quotaMetric": "generativelanguage.googleapis.com/generate_content_free_tier_input_token_count",
            "quotaId": "GenerateContentInputTokensPerModelPerMinute-FreeTier",
            "quotaDimensions": {
              "model": "gemini-3.1-pro",
              "location": "global"
            }
          },
          {
            "description": "Generate content input tokens per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.",
            "quotaMetric": "generativelanguage.googleapis.com/generate_content_free_tier_input_token_count",
            "quotaId": "GenerateContentInputTokensPerModelPerDay-FreeTier",
            "quotaDimensions": {
              "location": "global",
              "model": "gemini-3.1-pro"
            }
          }
        ]
      },
      {
        "@type": "type.googleapis.com/google.rpc.RetryInfo",
        "retryDelay": "70695s"
      }
    ]
  }
}"####;

/// The same on the compatible endpoint: the whole body is a JSON ARRAY (HTTP 429). Violation order differs from the native reply.
pub(crate) const FREE_429_COMPAT: &str = r####"[{
  "error": {
    "code": 429,
    "message": "You exceeded your current quota, please check your plan and billing details. For more information on this error, head to: https://aistudio.google.com/docs/rate-limits. To monitor your current usage, head to: https://ai.dev/rate-limit. \n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_input_token_count, limit: 0, model: gemini-3.1-pro, description: Generate content input tokens per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_requests, limit: 0, model: gemini-3.1-pro, description: Generate content requests per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_requests, limit: 0, model: gemini-3.1-pro, description: Generate content requests per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_input_token_count, limit: 0, model: gemini-3.1-pro, description: Generate content input tokens per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\nPlease retry in 19h38m14.824741283s.",
    "status": "RESOURCE_EXHAUSTED",
    "details": [
      {
        "@type": "type.googleapis.com/google.rpc.Help",
        "links": [
          {
            "description": "Learn more about Gemini API quotas",
            "url": "https://aistudio.google.com/docs/rate-limits"
          }
        ]
      },
      {
        "@type": "type.googleapis.com/google.rpc.QuotaFailure",
        "violations": [
          {
            "description": "Generate content input tokens per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.",
            "quotaMetric": "generativelanguage.googleapis.com/generate_content_free_tier_input_token_count",
            "quotaId": "GenerateContentInputTokensPerModelPerMinute-FreeTier",
            "quotaDimensions": {
              "location": "global",
              "model": "gemini-3.1-pro"
            }
          },
          {
            "description": "Generate content requests per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.",
            "quotaMetric": "generativelanguage.googleapis.com/generate_content_free_tier_requests",
            "quotaId": "GenerateRequestsPerMinutePerProjectPerModel-FreeTier",
            "quotaDimensions": {
              "model": "gemini-3.1-pro",
              "location": "global"
            }
          },
          {
            "description": "Generate content requests per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.",
            "quotaMetric": "generativelanguage.googleapis.com/generate_content_free_tier_requests",
            "quotaId": "GenerateRequestsPerDayPerProjectPerModel-FreeTier",
            "quotaDimensions": {
              "location": "global",
              "model": "gemini-3.1-pro"
            }
          },
          {
            "description": "Generate content input tokens per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.",
            "quotaMetric": "generativelanguage.googleapis.com/generate_content_free_tier_input_token_count",
            "quotaId": "GenerateContentInputTokensPerModelPerDay-FreeTier",
            "quotaDimensions": {
              "location": "global",
              "model": "gemini-3.1-pro"
            }
          }
        ]
      },
      {
        "@type": "type.googleapis.com/google.rpc.RetryInfo",
        "retryDelay": "70694s"
      }
    ]
  }
}
]"####;

/// HTTP 503 "high demand", native.
pub(crate) const BUSY_503_NATIVE: &str = r####"{
  "error": {
    "code": 503,
    "message": "This model is currently experiencing high demand. Spikes in demand are usually temporary. Please try again later.",
    "status": "UNAVAILABLE"
  }
}"####;

/// HTTP 503 on the compatible endpoint, array-wrapped.
pub(crate) const BUSY_503_COMPAT: &str = r####"[{
  "error": {
    "code": 503,
    "message": "This model is currently experiencing high demand. Spikes in demand are usually temporary. Please try again later.",
    "status": "UNAVAILABLE"
  }
}
]"####;

/// The paid key on 3.1 Pro, native (HTTP 200): the probe's Paid shape.
pub(crate) const PAID_NATIVE_PRO: &str = r####"{
  "candidates": [
    {
      "content": {},
      "finishReason": "MAX_TOKENS",
      "index": 0
    }
  ],
  "usageMetadata": {
    "promptTokenCount": 4,
    "totalTokenCount": 33,
    "promptTokensDetails": [
      {
        "modality": "TEXT",
        "tokenCount": 4
      }
    ],
    "thoughtsTokenCount": 29,
    "serviceTier": "standard"
  },
  "modelVersion": "gemini-3.1-pro-preview",
  "responseId": "TSUIa9P7Bpf-vdIPhuOosQE"
}"####;

/// The paid key on 3.1 Pro, compatible endpoint (HTTP 200): all thinking, no `content` key, completion 0 but total 34.
pub(crate) const PAID_USAGE_LENGTH: &str = r####"{"choices":[{"finish_reason":"length","index":0,"message":{"role":"assistant"}}],"created":1795695954,"id":"UCUIa9THE9zjnsEP_snM-Qk","model":"gemini-3.1-pro-preview","object":"chat.completion","usage":{"completion_tokens":0,"prompt_tokens":5,"total_tokens":34}}"####;

/// The paid key on 3.8 Flash, native (HTTP 200).
pub(crate) const PAID_NATIVE_FLASH: &str = r####"{
  "candidates": [
    {
      "content": {},
      "finishReason": "MAX_TOKENS",
      "index": 0
    }
  ],
  "usageMetadata": {
    "promptTokenCount": 4,
    "totalTokenCount": 32,
    "promptTokensDetails": [
      {
        "modality": "TEXT",
        "tokenCount": 4
      }
    ],
    "thoughtsTokenCount": 28,
    "serviceTier": "standard"
  },
  "modelVersion": "gemini-3.8-flash",
  "responseId": "UyUIa6uxAsjZxs0PhvnIsAE"
}"####;

/// The paid key on 3.8 Flash, compatible endpoint (HTTP 200): completion 1 but total 32.
pub(crate) const PAID_USAGE_OK: &str = r####"{"choices":[{"finish_reason":"stop","index":0,"message":{"content":"OK","extra_content":{"google":{"thought_signature":"EvoBCvcBAWkUfRNQPtK4oDdtaHUs//CUWo5ahPA/00/G5tpEHyvnhIOKz+ZNDJGxHCxGKIB5JKaCmk5nb/Fo5qO1VWFZNp5z8kX1JBFCy8YjU+D9NJEeR7o5MXy1+EnLYfxM3y2dNuZQZc5A8OZS6gzPrLV8eBtRpMGy/839XFNhF9NZ8/QUPpcEzL9DKwDVfKQxIjvE0mxm5zLp7UfodzBJZg+bk6ivIt/ejEA0tKfB/pB8XODMFyhiGAWh6BE/U5e9o3CcsIA4ZioLl2cKNJic9v7uLiHE25IRxYJoSDQqCH4kgWqmfbjycdScXP4NoLu6ItHW929nkh27wA=="}},"role":"assistant"}}],"created":1795695959,"id":"VSUIa4jyJMjrkdUPkNX3mA8","model":"gemini-3.8-flash","object":"chat.completion","usage":{"completion_tokens":1,"prompt_tokens":5,"total_tokens":32}}"####;

/// Build day step 2, paid key: the probe on 3.1 Pro (native, HTTP 200). An empty reply cut at MAX_TOKENS, `candidates` AND `usageMetadata` with `serviceTier: "standard"`, and no `candidatesTokenCount`.
pub(crate) const PAID_PROBE_200_PRO: &str = r####"{
  "candidates": [
    {
      "content": {
        "parts": [
          {
            "text": ""
          }
        ],
        "role": "model"
      },
      "finishReason": "MAX_TOKENS",
      "index": 0
    }
  ],
  "usageMetadata": {
    "promptTokenCount": 1,
    "totalTokenCount": 1,
    "promptTokensDetails": [
      {
        "modality": "TEXT",
        "tokenCount": 1
      }
    ],
    "serviceTier": "standard"
  },
  "modelVersion": "gemini-3.1-pro-preview",
  "responseId": "KmAIa86GHIbrkdUPtKWroA8"
}"####;

/// Build day step 2, paid key: the probe on Gemini 4 Carbon (HTTP 200).
pub(crate) const PAID_PROBE_200_CARBON: &str = r####"{
  "candidates": [
    {
      "content": {
        "parts": [
          {
            "text": ""
          }
        ],
        "role": "model"
      },
      "finishReason": "MAX_TOKENS",
      "index": 0
    }
  ],
  "usageMetadata": {
    "promptTokenCount": 32,
    "candidatesTokenCount": 1,
    "totalTokenCount": 33,
    "promptTokensDetails": [
      {
        "modality": "TEXT",
        "tokenCount": 32
      }
    ],
    "candidatesTokensDetails": [
      {
        "modality": "TEXT",
        "tokenCount": 1
      }
    ],
    "serviceTier": "standard"
  },
  "modelVersion": "gemini-4-carbon",
  "responseId": "LGAIa4jbL7aokdUPs93psQo"
}"####;

/// Build day step 2, paid key: the probe on Gemini 4 Argon (HTTP 200).
pub(crate) const PAID_PROBE_200_ARGON: &str = r####"{
  "candidates": [
    {
      "content": {
        "parts": [
          {
            "text": ""
          }
        ],
        "role": "model"
      },
      "finishReason": "MAX_TOKENS",
      "index": 0
    }
  ],
  "usageMetadata": {
    "promptTokenCount": 32,
    "candidatesTokenCount": 1,
    "totalTokenCount": 33,
    "promptTokensDetails": [
      {
        "modality": "TEXT",
        "tokenCount": 32
      }
    ],
    "candidatesTokensDetails": [
      {
        "modality": "TEXT",
        "tokenCount": 1
      }
    ],
    "serviceTier": "standard"
  },
  "modelVersion": "gemini-4-argon",
  "responseId": "LmAIa42qKYTavdIPnM6PmAc"
}"####;

/// Build day step 2, paid key: the probe on Gemini 4 Neon (HTTP 200).
pub(crate) const PAID_PROBE_200_NEON: &str = r####"{
  "candidates": [
    {
      "content": {
        "parts": [
          {
            "text": ""
          }
        ],
        "role": "model"
      },
      "finishReason": "MAX_TOKENS",
      "index": 0,
      "continuationToken": "(Google's opaque token, blanked: PM never reads it, and the secret scan flags it)"
    }
  ],
  "usageMetadata": {
    "promptTokenCount": 35,
    "totalTokenCount": 36,
    "promptTokensDetails": [
      {
        "modality": "TEXT",
        "tokenCount": 35
      }
    ],
    "thoughtsTokenCount": 1,
    "serviceTier": "standard"
  },
  "modelVersion": "gemini-4-neon",
  "responseId": "NGAIa525Bv-zkdUPkbXG8Q8"
}"####;

/// Build day step 2, free key: the probe on 3.1 Pro (native, HTTP 429, every quota `-FreeTier`, limit 0).
pub(crate) const FREE_PROBE_429_PRO: &str = r####"{
  "error": {
    "code": 429,
    "message": "You exceeded your current quota, please check your plan and billing details. For more information on this error, head to: https://aistudio.google.com/docs/rate-limits. To monitor your current usage, head to: https://ai.dev/rate-limit. \n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_input_token_count, limit: 0, model: gemini-3.1-pro, description: Generate content input tokens per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_requests, limit: 0, model: gemini-3.1-pro, description: Generate content requests per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_requests, limit: 0, model: gemini-3.1-pro, description: Generate content requests per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_input_token_count, limit: 0, model: gemini-3.1-pro, description: Generate content input tokens per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\nPlease retry in 15h19m33.440610728s.",
    "status": "RESOURCE_EXHAUSTED",
    "details": [
      {
        "@type": "type.googleapis.com/google.rpc.Help",
        "links": [
          {
            "description": "Learn more about Gemini API quotas",
            "url": "https://aistudio.google.com/docs/rate-limits"
          }
        ]
      },
      {
        "@type": "type.googleapis.com/google.rpc.QuotaFailure",
        "violations": [
          {
            "description": "Generate content input tokens per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.",
            "quotaMetric": "generativelanguage.googleapis.com/generate_content_free_tier_input_token_count",
            "quotaId": "GenerateContentInputTokensPerModelPerMinute-FreeTier",
            "quotaDimensions": {
              "location": "global",
              "model": "gemini-3.1-pro"
            }
          },
          {
            "description": "Generate content requests per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.",
            "quotaMetric": "generativelanguage.googlea"####;

/// Build day step 2, free key: the probe on Gemini 4 Carbon (HTTP 429, limit 0).
pub(crate) const FREE_PROBE_429_CARBON: &str = r####"{
  "error": {
    "code": 429,
    "message": "You exceeded your current quota, please check your plan and billing details. For more information on this error, head to: https://aistudio.google.com/docs/rate-limits. To monitor your current usage, head to: https://ai.dev/rate-limit. \n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_input_token_count, limit: 0, model: gemini-4-carbon, description: Generate content input tokens per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_requests, limit: 0, model: gemini-4-carbon, description: Generate content requests per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_requests, limit: 0, model: gemini-4-carbon, description: Generate content requests per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_input_token_count, limit: 0, model: gemini-4-carbon, description: Generate content input tokens per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\nPlease retry in 15h19m33.060996005s.",
    "status": "RESOURCE_EXHAUSTED",
    "details": [
      {
        "@type": "type.googleapis.com/google.rpc.Help",
        "links": [
          {
            "description": "Learn more about Gemini API quotas",
            "url": "https://aistudio.google.com/docs/rate-limits"
          }
        ]
      },
      {
        "@type": "type.googleapis.com/google.rpc.QuotaFailure",
        "violations": [
          {
            "description": "Generate content input tokens per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.",
            "quotaMetric": "generativelanguage.googleapis.com/generate_content_free_tier_input_token_count",
            "quotaId": "GenerateContentInputTokensPerModelPerDay-FreeTier",
            "quotaDimensions": {
              "location": "global",
              "model": "gemini-4-carbon"
            }
          },
          {
            "description": "Generate content requests per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.",
            "quotaMetric": "generativelanguage.googleapis."####;

/// Build day step 2, free key: the probe on Gemini 4 Argon (HTTP 429, limit 0).
pub(crate) const FREE_PROBE_429_ARGON: &str = r####"{
  "error": {
    "code": 429,
    "message": "You exceeded your current quota, please check your plan and billing details. For more information on this error, head to: https://aistudio.google.com/docs/rate-limits. To monitor your current usage, head to: https://ai.dev/rate-limit. \n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_input_token_count, limit: 0, model: gemini-4-argon, description: Generate content input tokens per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_requests, limit: 0, model: gemini-4-argon, description: Generate content requests per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_requests, limit: 0, model: gemini-4-argon, description: Generate content requests per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_input_token_count, limit: 0, model: gemini-4-argon, description: Generate content input tokens per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\nPlease retry in 15h19m32.621374777s.",
    "status": "RESOURCE_EXHAUSTED",
    "details": [
      {
        "@type": "type.googleapis.com/google.rpc.Help",
        "links": [
          {
            "description": "Learn more about Gemini API quotas",
            "url": "https://aistudio.google.com/docs/rate-limits"
          }
        ]
      },
      {
        "@type": "type.googleapis.com/google.rpc.QuotaFailure",
        "violations": [
          {
            "description": "Generate content input tokens per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.",
            "quotaMetric": "generativelanguage.googleapis.com/generate_content_free_tier_input_token_count",
            "quotaId": "GenerateContentInputTokensPerModelPerMinute-FreeTier",
            "quotaDimensions": {
              "location": "global",
              "model": "gemini-4-argon"
            }
          },
          {
            "description": "Generate content requests per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.",
            "quotaMetric": "generativelanguage.googlea"####;

/// Build day step 2, free key: the probe on Gemini 4 Neon (HTTP 429, limit 0).
pub(crate) const FREE_PROBE_429_NEON: &str = r####"{
  "error": {
    "code": 429,
    "message": "You exceeded your current quota, please check your plan and billing details. For more information on this error, head to: https://aistudio.google.com/docs/rate-limits. To monitor your current usage, head to: https://ai.dev/rate-limit. \n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_input_token_count, limit: 0, model: gemini-4-neon, description: Generate content input tokens per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_input_token_count, limit: 0, model: gemini-4-neon, description: Generate content input tokens per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_requests, limit: 0, model: gemini-4-neon, description: Generate content requests per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_requests, limit: 0, model: gemini-4-neon, description: Generate content requests per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\nPlease retry in 15h19m32.286896186s.",
    "status": "RESOURCE_EXHAUSTED",
    "details": [
      {
        "@type": "type.googleapis.com/google.rpc.Help",
        "links": [
          {
            "description": "Learn more about Gemini API quotas",
            "url": "https://aistudio.google.com/docs/rate-limits"
          }
        ]
      },
      {
        "@type": "type.googleapis.com/google.rpc.QuotaFailure",
        "violations": [
          {
            "description": "Generate content input tokens per day limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.",
            "quotaMetric": "generativelanguage.googleapis.com/generate_content_free_tier_input_token_count",
            "quotaId": "GenerateContentInputTokensPerModelPerDay-FreeTier",
            "quotaDimensions": {
              "location": "global",
              "model": "gemini-4-neon"
            }
          },
          {
            "description": "Generate content input tokens per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.",
            "quotaMetric": "generativelanguage.googleapis.com"####;

/// Build day step 4, free key: a real exhausted free allowance on 3.1 Flash-Lite after 32 calls (compatible endpoint, array-wrapped, HTTP 429). `quotaValue` 15, a `RetryInfo` of 28s, and NO "limit: 0".
pub(crate) const FREE_LIMIT_15_429_COMPAT: &str = r####"[{
  "error": {
    "code": 429,
    "message": "You exceeded your current quota, please check your plan and billing details. For more information on this error, head to: https://aistudio.google.com/docs/rate-limits. To monitor your current usage, head to: https://ai.dev/rate-limit. \n* Quota exceeded for metric: generativelanguage.googleapis.com/generate_content_free_tier_requests, limit: 15, model: gemini-3.1-flash-lite, description: Generate content requests per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.\nPlease retry in 28.997034408s.",
    "status": "RESOURCE_EXHAUSTED",
    "details": [
      {
        "@type": "type.googleapis.com/google.rpc.Help",
        "links": [
          {
            "description": "Learn more about Gemini API quotas",
            "url": "https://aistudio.google.com/docs/rate-limits"
          }
        ]
      },
      {
        "@type": "type.googleapis.com/google.rpc.QuotaFailure",
        "violations": [
          {
            "description": "Generate content requests per minute limit exceeded on Free Tier. See https://aistudio.google.com/docs/rate-limits.",
            "quotaMetric": "generativelanguage.googleapis.com/generate_content_free_tier_requests",
            "quotaId": "GenerateRequestsPerMinutePerProjectPerModel-FreeTier",
            "quotaDimensions": {
              "model": "gemini-3.1-flash-lite",
              "location": "global"
            },
            "quotaValue": "15"
          }
        ]
      },
      {
        "@type": "type.googleapis.com/google.rpc.RetryInfo",
        "retryDelay": "28s"
      }
    ]
  }
}
]"####;

/// Build day step 5: `minimal` on Gemini 4 Hydrogen (compatible endpoint, array-wrapped, HTTP 400).
pub(crate) const EFFORT_400_MINIMAL_HYDROGEN: &str = r####"[{
  "error": {
    "code": 400,
    "message": "Thinking level MINIMAL is not supported for this model. Please retry with other thinking level.",
    "status": "INVALID_ARGUMENT"
  }
}
]"####;

/// Build day step 5: `xhigh` on Gemini 4 Carbon (array-wrapped, HTTP 400).
pub(crate) const EFFORT_400_XHIGH_CARBON: &str = r####"[{
  "error": {
    "code": 400,
    "message": "Thinking level XHIGH is not supported for this model. Please retry with other thinking level.",
    "status": "INVALID_ARGUMENT"
  }
}
]"####;

/// Build day step 5: `none` on 3.1 Pro (array-wrapped, HTTP 400, "Budget 0 is invalid").
pub(crate) const EFFORT_400_NONE_PRO: &str = r####"[{
  "error": {
    "code": 400,
    "message": "Budget 0 is invalid. This model only works in thinking mode.",
    "status": "INVALID_ARGUMENT"
  }
}
]"####;

/// Build day step 5: `none` on 3.5 Flash-Lite (array-wrapped, HTTP 400, the generic "invalid argument").
pub(crate) const EFFORT_400_NONE_LITE_35: &str = r####"[{
  "error": {
    "code": 400,
    "message": "Request contains an invalid argument.",
    "status": "INVALID_ARGUMENT"
  }
}
]"####;

/// Build day step 9: `store: false` (array-wrapped, HTTP 400): the Gemini body must never carry `store`.
pub(crate) const STORE_400: &str = r####"[{
  "error": {
    "code": 400,
    "message": "Invalid JSON payload received. Unknown name \"store\": Cannot find field.",
    "status": "INVALID_ARGUMENT",
    "details": [
      {
        "@type": "type.googleapis.com/google.rpc.BadRequest",
        "fieldViolations": [
          {
            "description": "Invalid JSON payload received. Unknown name \"store\": Cannot find field."
          }
        ]
      }
    ]
  }
}
]"####;

/// Build day step 10: a made-up key on the native list (a plain object, HTTP 400, `API_KEY_INVALID`).
pub(crate) const BAD_KEY_NATIVE_LIST_400: &str = r####"{
  "error": {
    "code": 400,
    "message": "API key not valid. Please pass a valid API key.",
    "status": "INVALID_ARGUMENT",
    "details": [
      {
        "@type": "type.googleapis.com/google.rpc.ErrorInfo",
        "reason": "API_KEY_INVALID",
        "domain": "googleapis.com",
        "metadata": {
          "service": "generativelanguage.googleapis.com"
        }
      },
      {
        "@type": "type.googleapis.com/google.rpc.LocalizedMessage",
        "locale": "en-US",
        "message": "API key not valid. Please pass a valid API key."
      }
    ]
  }
}"####;

/// Build day step 10: a made-up key on the compatible `/models` (a plain object, HTTP 400).
pub(crate) const BAD_KEY_COMPAT_MODELS_400: &str = r####"{
  "error": {
    "code": 400,
    "message": "Please pass a valid API key",
    "status": "INVALID_ARGUMENT"
  }
}"####;

/// Build day step 10: a made-up key on the compatible chat (array-wrapped, HTTP 400).
pub(crate) const BAD_KEY_COMPAT_CHAT_400: &str = r####"[{
  "error": {
    "code": 400,
    "message": "Please pass a valid API key",
    "status": "INVALID_ARGUMENT"
  }
}
]"####;

/// Build day step 6: the last two chunks and `[DONE]` of a streamed chat on 3.8 Flash with `include_usage`. Every chunk carries `usage` (prompt 32, completion 3, total 150): 115 tokens of thinking are visible only as total minus prompt minus completion.
pub(crate) const STREAM_STEP6: &str = r####"data: {"choices":[{"delta":{"content":"18 pounds","role":"assistant"},"index":0}],"created":1795711149,"id":"q2AIa9OpAoj6xs0Pt7u86QY","model":"gemini-3.8-flash","object":"chat.completion.chunk","usage":{"completion_tokens":3,"prompt_tokens":32,"total_tokens":150}}

data: {"choices":[{"delta":{"extra_content":{"google":{"thought_signature":"EqkECqYEAWkUfRNkKDDNbrGEn43vI6An5csxctgD0Q2EskEbVorTHTIOwWtBMVaiJ6INnLuaq4GLkzG5oakGSO2xN9aWK/8vHKCJmIiRTF3yrYYVfrf4tENNa6dkJtHUQvCf/povq1S9um67GsXvNic93+BZqcESF204xFpRUDrIdsGhIDACzx3fLiI8xJResq6RhmdqzPVOrD8eS4H1MPZ6PUpC1FQLyabgUesXZBXSR8hY4iTYhrHQxDVdf0z8+yIrdgeL1Aw2PCw60HX0fGfW4iP+IC45t5GlXs5XmjFymqyWvVOSPyckbs4QIX93O2cdSphTlhJ9aymF8rwjclz3uY5LNcwgJOO7rw4qsHcVqAbrwRGrj2YSttnl3WZtgwfFVQjnbk+0mXvZgPFDt222F0feRu4ZmMtKJK4BFyn0b90gdp6ePdYTG/2zCPyg2Rh5LyZMTv/8XENNw4XothJDFtUD0btVXBSqu8PMiQKGRkBDQUEQzHpvRG72IX3czIhg9XLZfq/PQa6XjwruIsHnRCHHZYCOJRYF3l/7daTOQaxIcgtKopgTEP3QzNCHFXYRvX+tpJqaCtaQVpGDbjuByjjGiMJgOd1/RkFZODOmOHi6I4yI+oxkIboM64DPr+xOZxUKfvWoEEpHvcJ4bxZiKa+S4Ih79cCYBgynsEQBJ4hute/+n1fLmhC+fFKzXNiUoaLHw3RaPw0fl9/aEFiNHT6obAplyaemsQ=="}},"role":"assistant"},"finish_reason":"stop","index":0}],"created":1795711149,"id":"q2AIa9OpAoj6xs0Pt7u86QY","model":"gemini-3.8-flash","object":"chat.completion.chunk","usage":{"completion_tokens":3,"prompt_tokens":32,"total_tokens":150}}

data: [DONE]"####;

/// Build day step 7: a ~6.9K-token prompt on 3.8 Flash, first call (HTTP 200): no `prompt_tokens_details` at all.
pub(crate) const PAID_CACHED_FIRST: &str = r####"{"choices":[{"finish_reason":"stop","index":0,"message":{"content":"OK.","extra_content":{"google":{"thought_signature":"Ep8BCpwBAWkUfRMxCS70gqan10ianyivF92RsDXei7isPIYkOJIkb3DI5X7bms/nDNMzXjqgo87vIJR5gGd57Q6mxYISp52x9EfeXTZvtMUFId9RMy+xR4A5OmiQaMdpkjhbjr+IVhoVb7+2rqd+mvIDrivqKzZgmlTbwRZ7LDdlimwyM4OU1j0EBQmPgouNpUklmvYYf7p2oNzgx1P/2wMc"}},"role":"assistant"}}],"created":1795711152,"id":"rWAIa8yiNrW5kdUP2MDfEA","model":"gemini-3.8-flash","object":"chat.completion","usage":{"completion_tokens":2,"prompt_tokens":6906,"total_tokens":6908}}"####;

/// Build day step 7: the SAME prompt again (HTTP 200): `cached_tokens` 4861 INSIDE `prompt_tokens` 6906.
pub(crate) const PAID_CACHED_SECOND: &str = r####"{"choices":[{"finish_reason":"stop","index":0,"message":{"content":"OK.","extra_content":{"google":{"thought_signature":"Ep8BCpwBAWkUfRM2ynZddK+2lnEbziXGtXYtmCTmQFcQtEjVuNiqj/Dy99d8WwjlAmg0mDqMbSLhPWxGnw+sbCKY2U0IfgHNdUC2bilnyIioMKlBrafTN65BFW/dpoIFWiZoto6+hk5HKsN37iKn3iDJX59VJeu/bupd0sbGMiZ2e2MCiOHy3hLkjhqAzMUb7lK5ZE59swiZpgtIU7U6ISc1"}},"role":"assistant"}}],"created":1795711156,"id":"sGAIa5TTNqSkkdUPtZjPGA","model":"gemini-3.8-flash","object":"chat.completion","usage":{"completion_tokens":2,"prompt_tokens":6906,"prompt_tokens_details":{"cached_tokens":4861},"total_tokens":6908}}"####;
