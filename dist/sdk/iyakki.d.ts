// TypeScript declarations for window.iyakki, the bridge a tile app sees (SDK version 1.2; 1.0 was the original bridge, 1.1 added discovery, llm, models and speech.say).
// Compatibility (docs/development/SDK-BRIDGE-V2.md): a release only adds. Feature-detect with iyakki.has('name') before using a namespace newer than 1.0, and write
// `typeof iyakki.has === 'function' && iyakki.has(...)` if the app may run on an older IYAKKI. A name the host does not know rejects with code "Unsupported".
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
    // ---- SDK 1.2. New keys of a later SDK go here too, and in app.json "requires": {"capabilities_optional": [...]} so an older IYAKKI ignores them. ----
    /** iyakki.markdown: show formatted text. */
    markdown?: boolean;
    /** iyakki.embed: the kinds of viewer it may show inside its own page. */
    embed?: ("markdown" | "browser" | "media" | "dash")[];
    /** Web sites an embedded browser may visit (exact host names; your host policy also applies). */
    embed_hosts?: string[];
    /** iyakki.dash: the dashboard sources it may read (read only). */
    dash?: string[];
    /** iyakki.flows: the published flows it may start (each run asks). */
    flows?: string[];
    /** iyakki.k3s: "read" looks at its own part of the cluster, "manage" may also change it (every change asks). */
    k3s?: "read" | "manage";
    /** iyakki.media: list and play the person's media. */
    media?: boolean;
    /** iyakki.audio: play sound. */
    audio?: boolean;
    /** iyakki.speech.listen: listen through the microphone (a card every time; the app gets text only). */
    microphone?: boolean;
    /** iyakki.money: spend up to a daily limit the person sets, in this currency (starts at nothing). */
    money?: string;
  }

  /** The names of the boolean capabilities (the "network" list is separate). */
  type CapabilityName = "storage" | "files" | "clipboard" | "notifications" | "agent" | "llm" | "speech" | "markdown" | "embed" | "embed_hosts" | "dash" | "flows" | "k3s" | "media" | "audio" | "microphone" | "money";

  /** Why a call failed: Error.name is one of these where the host gives a code. */
  type ErrorName = "QuotaExceeded" | "Error";

  /** The codes of llm and speech calls (docs/development/SDK-API.md, section 3). They are on Error.code and are stable. */
  type ErrorCode = "NoCapability" | "Denied" | "BadArgument" | "QuotaExceeded" | "BudgetStop" | "Unavailable" | "Busy" | "Timeout" | "Blocked" | "Failed";

  /**
   * What the SDK calls reject with. Extra fields by code: BadArgument.field, QuotaExceeded.limit, BudgetStop.resets_at (unix seconds) with limit,
   * remaining and currency for money, Unavailable.fix, Busy.holder, Blocked.host, Unsupported.op / namespace / sdk (the host's SDK version).
   */
  interface SdkError extends Error {
    code: ErrorCode;
    field?: string;
    limit?: string;
    resets_at?: number;
    remaining?: number;
    currency?: string;
    fix?: string;
    holder?: string;
    host?: string;
    op?: string;
    namespace?: string;
    sdk?: string;
  }

  // ---- sdk ----
  /** Every reply of the SDK 1.2 namespaces is an object; `ignored` lists argument names this host did not know and ignored. */
  interface Ignorable {
    ignored?: string[];
  }
  interface CapabilityListing {
    sdk: string;
    major: number;
    minor: number;
    namespaces: { name: string; since: string; capability: string | null; ready: boolean; ops: { name: string; since: string; approval: "none" | "first-use" | "each-call" | "each-call+typed" | "limit"; effect: "read" | "own" | "change" | "spend" | "listen"; ready: boolean }[] }[];
    capabilities: { key: string; since: string; granted: boolean }[];
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
  interface ListenHandle {
    id: number;
    /** "result" gets {text} (the sound is never given to the app), "done" ends, "error" gets an SdkError. */
    on(event: "result", fn: (e: { text: string }) => void): ListenHandle;
    on(event: "done", fn: (e: { cancelled?: boolean }) => void): ListenHandle;
    on(event: "error", fn: (e: SdkError) => void): ListenHandle;
    off(event: string, fn: (...a: any[]) => void): ListenHandle;
    cancel(): Promise<{}>;
  }
  interface SpeechNamespace {
    /**
     * Needs: speech. Reads text aloud (up to 20000 characters, 30 a hour) and resolves once the sound has started.
     * The person is asked once, with the line that says where the text goes. Rejects with Busy (holder) when the speaker
     * cannot be shared, Unavailable without a voice, Blocked when the voice server is not allowed yet.
     */
    say(text: string, options?: { voice?: string; lang?: string; speed?: number }): Promise<{ id: number }>;
    /** Needs: speech. Stops one reading, or all of this app's. */
    stop(id?: number): Promise<void>;
    /**
     * SDK 1.2. Needs: microphone. A card every time names the microphone; max 60 seconds, 20 an hour, one at a time. Rejects with Unavailable
     * when there is no microphone or speech engine. The app gets text only.
     */
    listen(options?: { lang?: string; maxSeconds?: number; device?: string }): Promise<ListenHandle & Ignorable>;
  }

  // ---- markdown ----
  interface MarkdownNamespace {
    /** SDK 1.2. Needs: markdown. Analysis only: lines at a width (an estimate), the text without marks, the links. Text up to 1 MB. */
    render(text: string, options?: { width?: number }): Promise<{ lines: number; plain: string; truncated: boolean; links: { text: string; url: string }[]; blockedLinks: number; images: number } & Ignorable>;
    /** Needs: markdown. Opens the viewer as its own tile. */
    view(text: string, options?: { title?: string }): Promise<{ id: number } & Ignorable>;
    /** Needs: markdown. The person picks the file in the dialog; the app never sees a path. */
    openFile(): Promise<{ id: number } & Ignorable>;
  }

  // ---- embed ----
  interface EmbedHandle {
    id: number;
    update(options: object): Promise<{ id: number }>;
    /** markdown: scrollTo, find; browser: back, forward, reload, stop; media: play, pause, seek; dash: setRange; all: focus, blur. */
    call(method: string, args?: object): Promise<any>;
    unmount(): Promise<{}>;
    focus(): Promise<any>;
    blur(): Promise<any>;
    /** markdown: link {url}, scroll {y}, ready; browser: navigated {url,title}, loading, blocked {url,reason}, fullscreen, closed; media: state, ended, error; dash: range. */
    on(event: string, fn: (e: any) => void): EmbedHandle;
    off(event: string, fn: (e: any) => void): EmbedHandle;
  }
  interface EmbedNamespace {
    /**
     * SDK 1.2. Needs: embed (the kind). Shows a viewer in a rectangle of the page (an element id or {x,y,w,h}; 120x80 up to 900x2880; two at most, one browser).
     * A browser embed asks once and may only visit embed_hosts. The page never sees the pixels.
     */
    mount(target: string | { x: number; y: number; w: number; h: number }, kind: "markdown" | "browser" | "media" | "dash", options?: object): Promise<EmbedHandle>;
    list(): Promise<{ items: { id: number; kind: string }[] }>;
  }

  // ---- dash ----
  interface DashNamespace {
    /** SDK 1.2. Needs: dash. Only the sources named in the capability. */
    sources(): Promise<{ items: { name: string; kind: string; language: string }[] } & Ignorable>;
    /** Needs: dash. Read only; asks once per source; 1000 rows, 30 a minute. */
    query(source: string, query: string, options?: { range?: { from: string; to: string }; limit?: number }): Promise<{ columns: { name: string; type: string }[]; rows: any[][]; truncated: boolean } & Ignorable>;
    panel: {
      /** Needs: dash. A card every time. */
      add(dashboard: string, panel: { title: string; source: string; query: string; visual?: string }): Promise<{ id: string } & Ignorable>;
    };
  }

  // ---- flows ----
  interface FlowsNamespace {
    /** SDK 1.2. Needs: flows. Only the listed, published flows. */
    list(): Promise<{ items: { id: string; name: string; version: number; inputs: { name: string; type: string; required: boolean }[] }[] } & Ignorable>;
    /** Needs: flows. A card every time with the flow, its version and the inputs; 10 an hour. The same key returns the same run. */
    run(flow: string, inputs?: object, options?: { key?: string }): Promise<{ run: string; repeated?: boolean } & Ignorable>;
    /** Needs: flows. Only runs this app started. */
    status(run: string): Promise<{ state: string; steps: { node: string; state: string }[]; error: string | null } & Ignorable>;
    /** Needs: flows. A card every time. */
    cancel(run: string): Promise<{ run: string; state: string } & Ignorable>;
  }

  // ---- k3s ----
  interface K3sNamespace {
    /** SDK 1.2. Needs: k3s. */
    status(): Promise<{ cluster: string; namespace: string; ready: boolean; writable: boolean }>;
    /** Needs: k3s. Its own namespace only; Secret values are never returned (names only). */
    get(kind: string, options?: { name?: string; selector?: string }): Promise<{ items: object[] } & Ignorable>;
    logs(pod: string, options?: { tail?: number }): Promise<{ lines: string[] } & Ignorable>;
    /** Needs: k3s "manage". The card shows the plan; the cluster rules (no privileged, hostPath, ...) apply. */
    apply(yaml: string): Promise<{ plan: string } & Ignorable>;
    /** Needs: k3s "manage". A card every time. */
    restart(name: string): Promise<{ restarted: string } & Ignorable>;
  }

  // ---- media ----
  interface MediaNamespace {
    /** SDK 1.2. Needs: media. Titles only; asks once. */
    library(options?: { kind?: "audio" | "video" }): Promise<{ items: { id: string; title: string; artist?: string; secs: number; kind: string }[] } & Ignorable>;
    /** Needs: media. An id from the library plays at once; a file outside the person's media folders asks every time. */
    play(source: { id: string } | { file: string }, options?: { start?: number }): Promise<{ id: number } & Ignorable>;
    pause(id: number): Promise<{ id: number; state: string }>;
    resume(id: number): Promise<{ id: number; state: string }>;
    stop(id: number): Promise<{ id: number; state: string }>;
    state(id?: number): Promise<{ id: number | null; state: "playing" | "paused" | "ended" | "stopped"; position: number }>;
    queue: {
      add(id: string): Promise<{ queued: number }>;
      list(): Promise<{ items: object[] }>;
    };
  }

  // ---- audio ----
  interface AudioNamespace {
    /** SDK 1.2. Needs: audio. Asks once. One stream per app (a new one replaces the old); the person's volume and mute apply; Busy while the speaker cannot be shared. */
    play(source: { file: string } | { wav: string }, options?: { volume?: number; loop?: boolean; channel?: "cue" | "music" | "speech" }): Promise<{ id: number } & Ignorable>;
    pause(id: number): Promise<{ id: number }>;
    resume(id: number): Promise<{ id: number }>;
    stop(id: number): Promise<{ id: number }>;
    setVolume(id: number, volume: number): Promise<{ id: number }>;
    outputs(): Promise<{ items: { name: string; active: boolean }[] } & Ignorable>;
  }

  // ---- money ----
  interface MoneyNamespace {
    /** SDK 1.2. Needs: money. The limit is per app and per day, in the smallest unit of the declared currency, and starts at 0. */
    limits(): Promise<{ currency: string; perDayMinor: number; spentTodayMinor: number; remainingMinor: number; confirmEach: boolean; resetsAt: number }>;
    /**
     * Needs: money. Checked against the limit before anything else (rejects BudgetStop with limit, remaining and resets_at), then a card unless the person switched it
     * off. No payment provider is set up in 1.2: after the checks it rejects Unavailable and nothing is recorded.
     */
    spend(request: { amountMinor: number; currency: string; purpose: string; ref?: string }): Promise<{ receipt: string; remainingMinor: number; repeated?: boolean } & Ignorable>;
    /** Needs: money. A card every time; only the person's Allow raises the limit. */
    requestLimit(request: { perDayMinor: number; why: string }): Promise<{ perDayMinor: number } & Ignorable>;
  }

  // ---- the bridge ----
  interface Bridge {
    /** The SDK version of this host, like "1.2". */
    readonly version: string;
    /** True when this host knows the name ('markdown', 'markdown.render') at this version and the app declared what it needs. Absent on a host older than SDK 1.1. */
    has(name: string): boolean;
    capabilities(): CapabilityListing;
    /** The generic door: any call by name. A name this host does not know rejects with code "Unsupported"; arguments are the call's own. */
    call(op: string, args?: object): Promise<any>;
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
    readonly markdown: MarkdownNamespace;
    readonly embed: EmbedNamespace;
    readonly dash: DashNamespace;
    readonly flows: FlowsNamespace;
    readonly k3s: K3sNamespace;
    readonly media: MediaNamespace;
    readonly audio: AudioNamespace;
    readonly money: MoneyNamespace;
  }
}

declare const iyakki: Iyakki.Bridge;

interface Window {
  readonly iyakki: Iyakki.Bridge;
}
