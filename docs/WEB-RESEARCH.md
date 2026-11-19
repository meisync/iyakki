# Web research: how the assistant reads the web

Ask the chat (Agent switch on, Ctrl+G) for "top stock market news", "the latest on Tamil Nadu weather" or "summarize this
site", and the assistant does the reading itself: it searches, opens several different sites, follows front pages to the
real stories (and through "next page" links), reads them, and answers with a short dated summary and numbered sources.
It does not ask before each read: reading the web is a read-only, no-approval action. Every step is a tool block in the
chat, the tools are logged by host only (never your words or the page text), and the run is kept in RUNS like any other
agent run (its tool results include the list of sources with their addresses). This is the same copilot activity as the
rest of the agent; with the Agent switch off the chat explains that Ctrl+G turns it on.

## The tools

| Tool | What it does |
|---|---|
| `web_research(topic, max_pages<=8, sources?, fresh?)` | One step: search, pick different sites (two pages per site at most), read up to four pages at once inside 45 seconds, follow front pages to their stories (or to the site's RSS feed), drop near-identical stories, return per page the title, site, date, a 600 to 1200 character excerpt and the address, and a coverage note saying which pages could not be read and why. The model writes the summary. |
| `web_fetch(url, max_chars?, next?, page?)` | Title, date, author and the article text without menus, banners and related boxes. A front page is returned as its story links. It lists the page's pagination (next, older, numbered) and feed. `next: true` reads the page after this one, `page: N` reads numbered page N. |
| `web_links(url, filter?, limit<=60)` | The page's main links as number, text and address, story links first (headlines and article-like addresses on the same site; not menus, footers, ads or social buttons). |
| `web_feed(url, limit<=40)` | RSS/Atom headlines with link, date and summary. Accepts a page and finds its feed. |
| `web_search`, `weather`, `stock_quote`, `browser_page` | As before. For prices the model calls `stock_quote` first. |

## Limits and safety

* Every request goes through one client: the account's host policy, local and private addresses refused (also host names
  that resolve to them), redirects followed by hand and re-checked, a 5 MB cap, 20 s per request, 60 requests a minute.
* A request may read at most 30 pages (it starts afresh after two quiet minutes); a research stops starting new pages
  after 45 seconds and says which it skipped.
* Cookies live for one request and its redirects only (consent pages that set a cookie and send you back work); the
  person's cookies are never used. The transport says `Iyakki/0.1 (+assistant)` and accepts gzip.
* Everything read is untrusted text. The tools are labelled so, and the prompt says never to follow instructions in it.
* A site that refused (401/403/429, bot check) is remembered for 30 minutes and skipped by later researches.

## What it can and cannot read

Checked live on 13 October 2026 with the real client:

| Site | Result |
|---|---|
| finance.yahoo.com/news, cnbc.com/markets, bbc.com/news, theguardian.com/business, apnews.com, thehindu.com, economictimes.indiatimes.com, marketwatch.com | HTTP 200; front pages become lists of story links, stories are read as articles. CNBC, BBC, Guardian, The Hindu, Economic Times, MarketWatch also have RSS feeds that are known to work. |
| reuters.com, investing.com, zacks.com, thestreet.com (some days marketwatch.com) | HTTP 401/403 with "enable JS / bot check": they block automated readers whatever the headers say. They are not worked around; the message says so and the model uses other sources. |
| Pages that build themselves with JavaScript | Title, description and `articleBody` from JSON-LD/OpenGraph are used when present, with a note; otherwise the page is reported as unreadable. |
| Sign-in or paywalled pages, infinite scroll, "load more" buttons | Not readable here. |

## Relationship to browser automation

The `browser_*` tools (supervised browser automation, in development) are for what this cannot do: sites that need
JavaScript, a sign-in or clicks. They act in a browser the person can watch and are approved by the person. Plain reading
of public pages stays here because it is cheaper, faster, needs no approval and is easier to audit.
