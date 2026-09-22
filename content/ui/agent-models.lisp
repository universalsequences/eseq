;; Shared catalog for patch editor bubbles and agent conversations.
;; This is Lisp data (one quoted list of property lists), read from disk when
;; listing/selecting models or starting a new bubble/conversation. No rebuild
;; or restart is needed after editing it; reopen the model picker to refresh.
;;
;; Installed apps can override the entire catalog with agent-models.lisp in
;; the user Lisp root (~/.eseq.d by default; ESEQ_CONFIG_DIR overrides it in development).
;; Each provider needs exactly one :default and one :bubble-default model.
;; Providers: :openai, :gemini, :anthropic, :deepseek.
;; Capabilities: :balanced, :fast, :cheap. IDs must be exact API model IDs.
'((:id "gpt-5.5" :name "GPT-5.5"
   :provider :openai :capability :balanced :default true)
  (:id "gpt-6-astra" :name "GPT-6 Astra"
   :provider :openai :capability :balanced)
  (:id "gpt-5.6-luna" :name "GPT-5.6 Luna"
   :provider :openai :capability :cheap)
  (:id "gpt-5-mini" :name "GPT-5 mini"
   :provider :openai :capability :fast :bubble-default true)
  (:id "gpt-5-nano" :name "GPT-5 nano"
   :provider :openai :capability :cheap)
  (:id "gemini-3-flash-preview" :name "Gemini 3 Flash Preview"
   :provider :gemini :capability :cheap :default true)
  (:id "gemini-3.5-flash" :name "Gemini 3.5 Flash"
   :provider :gemini :capability :cheap :bubble-default true)
  (:id "gemini-2.5-pro" :name "Gemini 2.5 Pro"
   :provider :gemini :capability :balanced)
  (:id "gemini-2.5-flash" :name "Gemini 2.5 Flash"
   :provider :gemini :capability :cheap)
  (:id "gemini-2.5-flash-lite" :name "Gemini 2.5 Flash Lite"
   :provider :gemini :capability :cheap)
  (:id "claude-opus-5" :name "Claude Opus 5"
   :provider :anthropic :capability :balanced :default true)
  (:id "claude-fable-5" :name "Claude Fable 5"
   :provider :anthropic :capability :balanced)
  (:id "claude-fable-5-1" :name "Claude Fable 5.1"
   :provider :anthropic :capability :balanced)
  (:id "claude-sonnet-5" :name "Claude Sonnet 5"
   :provider :anthropic :capability :balanced)
  (:id "claude-haiku-4-5" :name "Claude Haiku 4.5"
   :provider :anthropic :capability :fast :bubble-default true)
  (:id "deepseek-v4-pro" :name "DeepSeek V4 Pro"
   :provider :deepseek :capability :balanced :default true)
  (:id "deepseek-v4-flash" :name "DeepSeek V4 Flash"
   :provider :deepseek :capability :fast :bubble-default true))
