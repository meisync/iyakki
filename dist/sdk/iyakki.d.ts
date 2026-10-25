// TypeScript declarations for window.iyakki, the bridge a tile app sees (SDK version 1.0 = the bridge as it ships today).
// Source of truth: docs/APP-AUTHORING.md ("The bridge window.iyakki"). The bridge is frozen; every argument is checked in
// the page and again on the host; at most 200 calls a second. A call whose capability the app did not declare in app.json
// throws an Error with the words "this app did not ask for ...", and the refusal is listed in the app's Info.
//
// How to use: keep this file in your project (iyakki sdk init copies it), or add it to tsconfig.json "files"/"include".
// One block per namespace below, each between "// ---- name ----" markers, so a new namespace is added as one more block.

declare namespace Iyakki {
  /** The capabilities an app can declare in app.json, under "capabilities". Everything is off unless declared. */
  interface Capabilities {
    /** iyakki.storage */
    storage?: boolean;
    /** iyakki.host.openFile / saveFile */
    files?: boolean;
    /** iyakki.clipboard */
    clipboard?: boolean;
    /** iyakki.notify */
    notifications?: boolean;
    /** iyakki.agent.ask (the person must allow it every time) */
    agent?: boolean;
    /** iyakki.llm and iyakki.models: a bare model through the person's own setup (asked once; daily limit) */
    llm?: boolean;
    /** iyakki.speech.say: reads text aloud with the person's voice settings (asked once) */
    speech?: boolean;
    /** Hosts the page may reach (also limited by the person's host policy). None by default. */
    network?: string[];
  }

  /** The names of the boolean capabilities (the "network" list is separate). */
  type CapabilityName = "storage" | "files" | "clipboard" | "notifications" | "agent" | "llm" | "speech";

  /** Why a call failed: Error.name is one of these where the host gives a code. */
  type ErrorName = "QuotaExceeded" | "Error";

  /** The codes of llm and speech calls (docs/development/SDK-API.md, section 3). They are on Error.code and are stable. */
  type ErrorCode = "NoCapability" | "Denied" | "BadArgument" | "QuotaExceeded" | "BudgetStop" | "Unavailable" | "Busy" | "Timeout" | "Blocked" | "Failed";

  /** What llm and speech calls reject with. Extra fields by code: BadArgument.field, QuotaExceeded.limit, BudgetStop.resets_at (unix seconds), Unavailable.fix, Busy.holder. */
  interface SdkError extends Error {
    code: ErrorCode;
    field?: string;
    limit?: string;
    resets_at?: number;
    fix?: string;
    holder?: string;
  }

  // ---- app ----
  interface AppNamespace {
    /** Needs: nothing. */
    info(): AppInfo;
  }
  interface AppInfo {
    id: string;
    name: string;
    version: string;
    publisher: string;
    /** The trust level shown in the bar (an unsigned dev app is marked UNSIGNED DEV APP). */
    trust: string;
    /** True when installed by developer mode (iyakki dev push / install). */
    dev: boolean;
    capabilities: Capabilities;
  }

  // ---- storage ----
  interface StorageNamespace {
    /**
     * The app's own data for this person. Needs: storage.
     * Keys: letters, digits and . _ : - (at most 64). Values: up to 64 KB each, 256 KB and 256 keys in all.
     * Past that, set() rejects with an error named QuotaExceeded.
     */
    get(key: string): Promise<string | null>;
    set(key: string, value: string): Promise<void>;
    remove(key: string): Promise<void>;
    list(): Promise<string[]>;
  }

  // ---- ui ----
  interface UiNamespace {
    /** Needs: nothing. A short message (at most 200 characters). */
    toast(text: string): void;
    /** Needs: nothing. A new title in the header (at most 60 characters). */
    title(text: string): void;
  }

  // ---- host ----
  interface HostNamespace {
    /** Needs: files. The person picks a text file (up to 256 KB) from their Uploads folder in a dialog. */
    openFile(): Promise<{ name: string; text: string }>;
    /** Needs: files. The person is asked and chooses where it is saved; the app never sees a path. */
    saveFile(name: string, text: string): Promise<void>;
  }

  // ---- clipboard ----
  interface ClipboardNamespace {
    /** Needs: clipboard. Each call asks the person. */
    read(): Promise<string>;
    write(text: string): Promise<void>;
  }

  // ---- notify ----
  interface NotifyNamespace {
    /** Needs: notifications. A note in the person's inbox (5 per run). */
    show(title: string, body: string): void;
  }

  // ---- agent ----
  interface AgentNamespace {
    /** Needs: agent. The person sees the question and must allow it every time; the assistant gets only the question. */
    ask(question: string): Promise<string>;
  }

  // ---- llm ----
  interface ChatMessage {
    role: "system" | "user" | "assistant";
    content: string;
  }
  interface ChatOptions {
    /** An id from models.list(). Omit it to use the person's chat model. */
    model?: string;
    /** 1 to 4096, default 1024. */
    max_tokens?: number;
    /** true: chat() returns a stream handle instead of a promise (same as llm.stream). */
    stream?: boolean;
  }
  interface ChatReply {
    text: string;
    /** Estimated is true when the provider gave no exact count (about 4 characters a token). */
    usage: { input: number; output: number; estimated: boolean };
    model: string;
    /** True when the stream was cancelled and this is the part written so far. */
    cancelled?: boolean;
  }
  interface ChatStream {
    /** "text" gets {text}, "done" gets the ChatReply, "error" gets an SdkError, "dropped" says events were lost (at most 50 wait). */
    on(event: "text", fn: (e: { text: string }) => void): ChatStream;
    on(event: "done", fn: (r: ChatReply) => void): ChatStream;
    on(event: "error", fn: (e: SdkError) => void): ChatStream;
    on(event: "dropped", fn: () => void): ChatStream;
    off(event: "text" | "done" | "error" | "dropped", fn: (...a: any[]) => void): ChatStream;
    /** Stops the model; the part written so far is still counted. */
    cancel(): void;
  }
  interface UsageNumbers {
    calls: number;
    input: number;
    output: number;
    errors: number;
    meanMs: number;
    estimated: boolean;
    /** Money is never claimed: cents is 0 and priced is false until a price is known. */
    cents: number;
    priced: boolean;
  }
  interface UsageReport {
    since: "today" | "7d" | "30d";
    today: UsageNumbers;
    range: UsageNumbers;
    /** null: no daily limit (a model on this computer, unless the person set one). */
    limits: { tokensPerDay: number | null; centsPerDay: number | null };
    remaining: { tokens: number | null; cents: number | null };
    /** Unix seconds when today's limit starts again. */
    resetsAt: number;
    byDay: { day: string; calls: number; input: number; output: number }[];
  }
  interface LlmNamespace {
    /**
     * Needs: llm. A bare model of the person's choice: no tools, no files, no memory. The person is asked once per app.
     * 20 calls a minute, prompt up to 100000 characters, 60 seconds. Rejects with BudgetStop at the daily limit, Unavailable
     * without a model, Denied if the person says no. Both call shapes work: chat({messages, ...}) and chat(messages, options).
     */
    chat(args: { messages: ChatMessage[] } & ChatOptions): Promise<ChatReply>;
    chat(messages: ChatMessage[], options?: Omit<ChatOptions, "stream">): Promise<ChatReply>;
    chat(args: { messages: ChatMessage[] } & ChatOptions & { stream: true }): ChatStream;
    /** Needs: llm. One stream at a time. Errors arrive as the "error" event. */
    stream(messages: ChatMessage[], options?: Omit<ChatOptions, "stream">): ChatStream;
    stream(args: { messages: ChatMessage[] } & Omit<ChatOptions, "stream">): ChatStream;
    /** Needs: llm. This app's own numbers only. */
    usage(options?: { since?: "today" | "7d" | "30d" }): Promise<UsageReport>;
  }

  // ---- models ----
  interface ModelsNamespace {
    /** Needs: llm. Ids and a where label only; the provider, its address and its key are never shown to the app. */
    list(options?: { kind?: "chat" | "embedding" | "image" | "stt" | "tts" }): Promise<{ id: string; label: string; kind: string; where: "this computer" | "network" | "internet"; free: boolean }[]>;
  }

  // ---- speech ----
  interface SpeechNamespace {
    /**
     * Needs: speech. Reads text aloud (up to 20000 characters, 30 a hour) and resolves once the sound has started.
     * The person is asked once, with the line that says where the text goes. Rejects with Busy (holder) when the speaker
     * cannot be shared, Unavailable without a voice, Blocked when the voice server is not allowed yet.
     */
    say(text: string, options?: { voice?: string; lang?: string; speed?: number }): Promise<{ id: number }>;
    /** Needs: speech. Stops one reading, or all of this app's. */
    stop(id?: number): Promise<void>;
  }

  // ---- the bridge ----
  interface Bridge {
    readonly app: AppNamespace;
    readonly storage: StorageNamespace;
    readonly ui: UiNamespace;
    readonly host: HostNamespace;
    readonly clipboard: ClipboardNamespace;
    readonly notify: NotifyNamespace;
    readonly agent: AgentNamespace;
    readonly llm: LlmNamespace;
    readonly models: ModelsNamespace;
    readonly speech: SpeechNamespace;
  }
}

declare const iyakki: Iyakki.Bridge;

interface Window {
  readonly iyakki: Iyakki.Bridge;
}
