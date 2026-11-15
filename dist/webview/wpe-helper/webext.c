/* The IYAKKI web process extension (libiyakkiwebext.so), loaded into every WPE web process. It runs the browser extensions' hooks
 * that must act inside the web process; today only `request.headers` (custom request headers, F-BROW-79). The shell hands the helper
 * an extension snapshot (crates/iyakki-webview/src/ext.rs, parsed by extsnap.h); the helper passes the bytes on as a WebKitUserMessage
 * ("iyakki-extensions", an array of bytes) and at web process start as the initialization user data. Hook kinds this build has no
 * code for are skipped and reported back ("iyakki-extensions-report"), never fatal.
 *
 * request.headers: on every `send-request` of a page (the page, its sub-resources and every redirect hop) the rules are asked about
 * the request address; matching headers are added unless the page set the header itself (unless the rule says replace). On a redirect
 * the headers this extension added to the previous hop are removed when the new address does not match, because the engine copies
 * the previous request's headers into the redirected one. Header values are never logged and never leave this process.
 *
 * NOT yet compiled or run against a real WPE WebKit: the build host has no WPE headers. The API used (WebKitWebExtension,
 * "page-created", "send-request", "user-message-received", SoupMessageHeaders) is the WPE WebKit 2.x / libsoup 3 API; see
 * KNOWN-ISSUES.md. */
#include <glib.h>
#include <gmodule.h>
#include <libsoup/soup.h>
#if __has_include(<wpe/webkit-web-extension.h>)
#include <wpe/webkit-web-extension.h>
#else
#include <wpe/webkit-web-process-extension.h>
#define WebKitWebExtension WebKitWebProcessExtension
#define webkit_web_extension_send_message_to_context webkit_web_process_extension_send_message_to_context
#endif

#include "extsnap.h"

static XHeaders X;
static WebKitWebExtension *EXT;

static void report(const char *text) {
    if (!EXT) return;
    WebKitUserMessage *m = webkit_user_message_new("iyakki-extensions-report", g_variant_new_string(text));
    webkit_web_extension_send_message_to_context(EXT, m, NULL, NULL, NULL);
}

static void load_snapshot(GVariant *v) {
    char rep[1024];
    gsize n = 0;
    gconstpointer d = v && g_variant_is_of_type(v, G_VARIANT_TYPE("ay")) ? g_variant_get_fixed_array(v, &n, 1) : NULL;
    if (!d || n == 0) { xs_free(&X); return; }
    int rc = xs_parse(d, n, &X, rep, sizeof rep);
    char line[1200];
    snprintf(line, sizeof line, "%s, %u header rules%s%s", rc < 0 ? "refused" : "loaded", X.n, rep[0] ? ": " : "", rep);
    report(line);
}

/* remove a header this extension added earlier (same name and value as a rule) when the rule does not apply to this hop */
static void strip_ours(SoupMessageHeaders *h, const XTarget *t, int target_ok) {
    for (uint32_t i = 0; i < X.n; i++) {
        XRule *r = &X.rules[i];
        const char *cur = soup_message_headers_get_one(h, r->header);
        if (cur && !strcmp(cur, r->value) && !(target_ok && xs_rule_matches(r, t))) soup_message_headers_remove(h, r->header);
    }
}

static gboolean on_send_request(WebKitWebPage *page, WebKitURIRequest *req, WebKitURIResponse *redirected, gpointer data) {
    if (X.n == 0) return FALSE;
    SoupMessageHeaders *h = webkit_uri_request_get_http_headers(req);
    if (!h) return FALSE;
    XTarget t;
    int ok = xs_target(webkit_uri_request_get_uri(req), &t) == 0;
    if (redirected) strip_ours(h, &t, ok);
    if (!ok) return FALSE;
    XOut out[XS_MAX_PER_REQUEST];
    int n = xs_headers_for(&X, &t, out);
    for (int i = 0; i < n; i++) {
        if (!out[i].replace && soup_message_headers_get_one(h, out[i].name)) continue;
        soup_message_headers_replace(h, out[i].name, out[i].value);
    }
    return FALSE;
}

static void on_page_created(WebKitWebExtension *ext, WebKitWebPage *page, gpointer data) {
    g_signal_connect(page, "send-request", G_CALLBACK(on_send_request), NULL);
}

static gboolean on_message(WebKitWebExtension *ext, WebKitUserMessage *m, gpointer data) {
    if (g_strcmp0(webkit_user_message_get_name(m), "iyakki-extensions") != 0) return FALSE;
    load_snapshot(webkit_user_message_get_parameters(m));
    return TRUE;
}

G_MODULE_EXPORT void webkit_web_extension_initialize_with_user_data(WebKitWebExtension *ext, GVariant *user_data) {
    EXT = ext;
    g_signal_connect(ext, "page-created", G_CALLBACK(on_page_created), NULL);
    g_signal_connect(ext, "user-message-received", G_CALLBACK(on_message), NULL);
    load_snapshot(user_data);
}
