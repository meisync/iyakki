/* The QuickJS-ng engine with a thin bridge to the host (see crates/iyakki-js). Compiled to WebAssembly by dist/js/build.sh.
 *
 * Everything the script can reach beyond the JavaScript language itself goes through one function, __host(module, op,
 * json), which the host answers. There is no file, network or clock access in here: the host decides what a module may do.
 */
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include "quickjs.h"

__attribute__((import_module("iyakki"), import_name("host_call")))
extern int32_t iyakki_host_call(const char *module, int32_t module_len, const char *op, int32_t op_len, const char *args, int32_t args_len);
__attribute__((import_module("iyakki"), import_name("host_result")))
extern void iyakki_host_result(char *out, int32_t cap);
__attribute__((import_module("iyakki"), import_name("interrupt")))
extern int32_t iyakki_interrupt(void);

static JSRuntime *rt;

static JSContext *ctx;
/* the last result or error text, for the host to read */
static char *out_buf;
static uint32_t out_len;

static void set_out(const char *s, size_t n) {
    free(out_buf);
    out_buf = malloc(n + 1);
    if (!out_buf) { out_len = 0; return; }
    memcpy(out_buf, s, n);
    out_buf[n] = 0;
    out_len = (uint32_t)n;
}

/* A sampling profiler for development: when on, every interrupt tick (about every 10 000 script operations) hands the
 * current script stack to the global function __prof (which the host sets up when asked to profile). */
static int prof_on;
static void profile_sample(void);

static int interrupt_cb(JSRuntime *r, void *opaque) {
    (void)r; (void)opaque;
    if (prof_on) profile_sample();
    int stop = iyakki_interrupt();
    if (stop && ctx) {
        /* say where the script was when it was stopped: the page's error list then shows the place, not just "too long" */
        static int told;
        if (!told) {
            told = 1;
            JSValue e = JS_NewError(ctx);
            JSValue st = JS_GetPropertyStr(ctx, e, "stack");
            const char *t = JS_ToCString(ctx, st);
            if (t) { fputs("stopped at:\n", stderr); fputs(t, stderr); fputc('\n', stderr); JS_FreeCString(ctx, t); }
            JS_FreeValue(ctx, st);
            JS_FreeValue(ctx, e);
        }
    }
    return stop;
}

static JSValue js_host(JSContext *c, JSValueConst this_val, int argc, JSValueConst *argv) {
    (void)this_val;
    if (argc < 3) return JS_ThrowTypeError(c, "__host needs (module, op, json)");
    size_t ml, ol, al;
    const char *m = JS_ToCStringLen(c, &ml, argv[0]);
    const char *o = JS_ToCStringLen(c, &ol, argv[1]);
    const char *a = JS_ToCStringLen(c, &al, argv[2]);
    if (!m || !o || !a) {
        if (m) JS_FreeCString(c, m);
        if (o) JS_FreeCString(c, o);
        if (a) JS_FreeCString(c, a);
        return JS_EXCEPTION;
    }
    int32_t n = iyakki_host_call(m, (int32_t)ml, o, (int32_t)ol, a, (int32_t)al);
    JS_FreeCString(c, m);
    JS_FreeCString(c, o);
    JS_FreeCString(c, a);
    int32_t len = n < 0 ? -n : n;
    char *buf = js_malloc(c, (size_t)len + 1);
    if (!buf) return JS_EXCEPTION;
    iyakki_host_result(buf, len);
    JSValue r;
    if (n < 0) r = JS_ThrowInternalError(c, "%.*s", len, buf);
    else r = JS_NewStringLen(c, buf, (size_t)len);
    js_free(c, buf);
    return r;
}

/* __eval(source, name): run a script in the global scope under its own name, so that errors and stack traces say
 * which file they came from (plain eval() calls everything "<input>"). The page uses it for scripts added at run time. */
static JSValue js_eval_named(JSContext *c, JSValueConst this_val, int argc, JSValueConst *argv) {
    (void)this_val;
    if (argc < 2) return JS_ThrowTypeError(c, "__eval needs (source, name)");
    size_t sl, nl;
    const char *src = JS_ToCStringLen(c, &sl, argv[0]);
    const char *name = JS_ToCStringLen(c, &nl, argv[1]);
    if (!src || !name) {
        if (src) JS_FreeCString(c, src);
        if (name) JS_FreeCString(c, name);
        return JS_EXCEPTION;
    }
    JSValue r = JS_Eval(c, src, sl, name, JS_EVAL_TYPE_GLOBAL);
    JS_FreeCString(c, src);
    JS_FreeCString(c, name);
    return r;
}

/* ---- ES modules: the host resolves specifiers and supplies the source text ---- */

static void json_escape(const char *s, char **out, size_t *olen) {
    size_t n = strlen(s), cap = n * 6 + 3, k = 0;
    char *b = malloc(cap);
    for (size_t i = 0; i < n; i++) {
        unsigned char ch = (unsigned char)s[i];
        if (ch == '"' || ch == '\\') { b[k++] = '\\'; b[k++] = (char)ch; }
        else if (ch < 0x20) { k += (size_t)snprintf(b + k, 7, "\\u%04x", ch); }
        else b[k++] = (char)ch;
    }
    b[k] = 0;
    *out = b;
    *olen = k;
}

/* Ask the host's "module" plug-in; the answer is js_malloc'd (NULL with a pending exception on failure). */
static char *module_call(JSContext *c, const char *op, const char *k1, const char *v1, const char *k2, const char *v2, size_t *rlen) {
    char *e1, *e2 = NULL; size_t l1, l2 = 0;
    json_escape(v1, &e1, &l1);
    if (v2) json_escape(v2, &e2, &l2);
    size_t cap = l1 + l2 + 64;
    char *args = malloc(cap);
    int an = v2 ? snprintf(args, cap, "{\"%s\":\"%s\",\"%s\":\"%s\"}", k1, e1, k2, e2) : snprintf(args, cap, "{\"%s\":\"%s\"}", k1, e1);
    int32_t n = iyakki_host_call("module", 6, op, (int32_t)strlen(op), args, an);
    free(args); free(e1); free(e2);
    int32_t len = n < 0 ? -n : n;
    char *buf = js_malloc(c, (size_t)len + 1);
    if (!buf) return NULL;
    iyakki_host_result(buf, len);
    buf[len] = 0;
    if (n < 0) { JS_ThrowReferenceError(c, "%.*s", len, buf); js_free(c, buf); return NULL; }
    if (rlen) *rlen = (size_t)len;
    return buf;
}

static char *module_normalize(JSContext *c, const char *base, const char *name, void *opaque) {
    (void)opaque;
    return module_call(c, "resolve", "base", base ? base : "", "name", name, NULL);
}

static void set_import_meta(JSContext *c, JSValueConst func_val, const char *url) {
    JSModuleDef *m = JS_VALUE_GET_PTR(func_val);
    JSValue meta = JS_GetImportMeta(c, m);
    if (JS_IsException(meta)) return;
    JS_DefinePropertyValueStr(c, meta, "url", JS_NewString(c, url), JS_PROP_C_W_E);
    JS_DefinePropertyValueStr(c, meta, "main", JS_NewBool(c, 0), JS_PROP_C_W_E);
    JS_FreeValue(c, meta);
}

static JSModuleDef *module_loader(JSContext *c, const char *name, void *opaque) {
    (void)opaque;
    size_t len = 0;
    char *src = module_call(c, "load", "name", name, NULL, NULL, &len);
    if (!src) return NULL;
    JSValue func_val = JS_Eval(c, src, len, name, JS_EVAL_TYPE_MODULE | JS_EVAL_FLAG_COMPILE_ONLY);
    js_free(c, src);
    if (JS_IsException(func_val)) return NULL;
    set_import_meta(c, func_val, name);
    JSModuleDef *m = JS_VALUE_GET_PTR(func_val);
    JS_FreeValue(c, func_val);
    return m;
}

static void profile_sample(void) {
    JSValue g = JS_GetGlobalObject(ctx);
    JSValue fn = JS_GetPropertyStr(ctx, g, "__prof");
    if (JS_IsFunction(ctx, fn)) {
        prof_on = 0; /* not while sampling */
        JSValue err = JS_NewError(ctx);
        JSValue st = JS_GetPropertyStr(ctx, err, "stack");
        JSValue r = JS_Call(ctx, fn, JS_UNDEFINED, 1, &st);
        JS_FreeValue(ctx, r);
        JS_FreeValue(ctx, st);
        JS_FreeValue(ctx, err);
        if (JS_HasException(ctx)) JS_FreeValue(ctx, JS_GetException(ctx));
        prof_on = 1;
    }
    JS_FreeValue(ctx, fn);
    JS_FreeValue(ctx, g);
}

__attribute__((export_name("jb_profile")))
void jb_profile(int32_t on) { prof_on = on != 0; }

/* the promise of the module run last */
static JSValue module_promise;

/* Describe the pending exception (message and stack) into the out buffer. */
static void take_exception(void) {
    JSValue ex = JS_GetException(ctx);
    const char *s = JS_ToCString(ctx, ex);
    JSValue stack = JS_GetPropertyStr(ctx, ex, "stack");
    const char *st = JS_IsUndefined(stack) ? NULL : JS_ToCString(ctx, stack);
    size_t sl = s ? strlen(s) : 0, stl = st ? strlen(st) : 0;
    char *both = malloc(sl + stl + 2);
    if (both) {
        memcpy(both, s ? s : "", sl);
        size_t n = sl;
        if (st && stl) { both[n++] = '\n'; memcpy(both + n, st, stl); n += stl; }
        set_out(both, n);
        free(both);
    }
    if (st) JS_FreeCString(ctx, st);
    if (s) JS_FreeCString(ctx, s);
    JS_FreeValue(ctx, stack);
    JS_FreeValue(ctx, ex);
}

__attribute__((export_name("jb_alloc"))) uint8_t *jb_alloc(uint32_t n) { return malloc(n ? n : 1); }
__attribute__((export_name("jb_free"))) void jb_free(uint8_t *p) { free(p); }
__attribute__((export_name("jb_out"))) const char *jb_out(void) { return out_buf ? out_buf : ""; }
__attribute__((export_name("jb_out_len"))) uint32_t jb_out_len(void) { return out_buf ? out_len : 0; }

/* Create the engine. 0 = ok. */
__attribute__((export_name("jb_init")))
int32_t jb_init(uint32_t mem_limit, uint32_t stack_limit) {
    if (rt) return 0;
    rt = JS_NewRuntime();
    if (!rt) return 1;
    if (mem_limit) JS_SetMemoryLimit(rt, mem_limit);
    if (stack_limit) JS_SetMaxStackSize(rt, stack_limit);
    JS_SetInterruptHandler(rt, interrupt_cb, NULL);
    JS_SetModuleLoaderFunc(rt, module_normalize, module_loader, NULL);
    module_promise = JS_UNDEFINED;
    ctx = JS_NewContext(rt);
    if (!ctx) return 2;
    JSValue g = JS_GetGlobalObject(ctx);
    JS_SetPropertyStr(ctx, g, "__host", JS_NewCFunction(ctx, js_host, "__host", 3));
    JS_SetPropertyStr(ctx, g, "__eval", JS_NewCFunction(ctx, js_eval_named, "__eval", 2));
    JS_FreeValue(ctx, g);
    return 0;
}

/* Run a script in the global scope. 0 = ok (the result as text is in the out buffer), 1 = it threw (the message is). */
__attribute__((export_name("jb_eval")))
int32_t jb_eval(const char *src, uint32_t len, const char *name, uint32_t name_len) {
    (void)name_len;
    if (!ctx) { set_out("engine not started", 18); return 1; }
    JSValue v = JS_Eval(ctx, src, len, name ? name : "<script>", JS_EVAL_TYPE_GLOBAL);
    if (JS_IsException(v)) { take_exception(); return 1; }
    size_t n = 0;
    const char *s = JS_IsUndefined(v) ? "" : JS_ToCStringLen(ctx, &n, v);
    set_out(s ? s : "", s ? n : 0);
    if (s && !JS_IsUndefined(v)) JS_FreeCString(ctx, s);
    JS_FreeValue(ctx, v);
    return 0;
}

/* Finish a script run: the value as text in the out buffer (0), or the exception (1). Consumes `v`. */
static int32_t finish_eval(JSValue v) {
    if (JS_IsException(v)) { take_exception(); return 1; }
    size_t n = 0;
    const char *s = JS_IsUndefined(v) ? "" : JS_ToCStringLen(ctx, &n, v);
    set_out(s ? s : "", s ? n : 0);
    if (s && !JS_IsUndefined(v)) JS_FreeCString(ctx, s);
    JS_FreeValue(ctx, v);
    return 0;
}

/* Compiled scripts, so that a big script is parsed once per machine instead of once per page load. */
static JSValue pending = JS_UNDEFINED;

/* Compile a script without running it. 0 = compiled (its bytecode is in the out buffer, and the script waits for
 * jb_run_pending), 1 = it did not compile (the message is). */
__attribute__((export_name("jb_compile")))
int32_t jb_compile(const char *src, uint32_t len, const char *name, uint32_t name_len) {
    (void)name_len;
    if (!ctx) { set_out("engine not started", 18); return 1; }
    JSValue f = JS_Eval(ctx, src, len, name ? name : "<script>", JS_EVAL_TYPE_GLOBAL | JS_EVAL_FLAG_COMPILE_ONLY);
    if (JS_IsException(f)) { take_exception(); return 1; }
    size_t n = 0;
    uint8_t *buf = JS_WriteObject(ctx, &n, f, JS_WRITE_OBJ_BYTECODE);
    if (!buf) { JS_FreeValue(ctx, f); set_out("could not store the compiled script", 36); return 1; }
    set_out((const char *)buf, n);
    js_free(ctx, buf);
    JS_FreeValue(ctx, pending);
    pending = f;
    return 0;
}

/* Run what jb_compile compiled. */
__attribute__((export_name("jb_run_pending")))
int32_t jb_run_pending(void) {
    JSValue f = pending;
    pending = JS_UNDEFINED;
    if (JS_IsUndefined(f)) { set_out("nothing compiled", 16); return 1; }
    return finish_eval(JS_EvalFunction(ctx, f));
}

/* Run a script from its bytecode. 0 = ok, 1 = it threw, 2 = the bytecode could not be read (compile the source). */
__attribute__((export_name("jb_eval_bytecode")))
int32_t jb_eval_bytecode(const uint8_t *buf, uint32_t len) {
    if (!ctx) { set_out("engine not started", 18); return 1; }
    JSValue f = JS_ReadObject(ctx, buf, len, JS_READ_OBJ_BYTECODE);
    if (JS_IsException(f)) { if (JS_HasException(ctx)) JS_FreeValue(ctx, JS_GetException(ctx)); return 2; }
    return finish_eval(JS_EvalFunction(ctx, f));
}

/* Run a module (the host has the address as its name, so relative imports resolve). 0 = started, 1 = it did not compile or link. */
__attribute__((export_name("jb_eval_module")))
int32_t jb_eval_module(const char *src, uint32_t len, const char *name, uint32_t name_len) {
    (void)name_len;
    if (!ctx) { set_out("engine not started", 18); return 1; }
    JSValue func_val = JS_Eval(ctx, src, len, name ? name : "<module>", JS_EVAL_TYPE_MODULE | JS_EVAL_FLAG_COMPILE_ONLY);
    if (JS_IsException(func_val)) { take_exception(); return 1; }
    set_import_meta(ctx, func_val, name ? name : "");
    JS_FreeValue(ctx, module_promise);
    module_promise = JS_EvalFunction(ctx, func_val);
    if (JS_IsException(module_promise)) { module_promise = JS_UNDEFINED; take_exception(); return 1; }
    return 0;
}

/* After the jobs have run: 0 = the module finished, 1 = it threw (the message is in the out buffer), 2 = still waiting. */
__attribute__((export_name("jb_module_status")))
int32_t jb_module_status(void) {
    if (JS_IsUndefined(module_promise)) return 0;
    switch (JS_PromiseState(ctx, module_promise)) {
    case JS_PROMISE_REJECTED: {
        JSValue reason = JS_PromiseResult(ctx, module_promise);
        JS_Throw(ctx, reason);
        take_exception();
        JS_FreeValue(ctx, module_promise);
        module_promise = JS_UNDEFINED;
        return 1;
    }
    case JS_PROMISE_PENDING: return 2;
    default: return 0;
    }
}

/* Run the queued promise jobs. Returns how many ran, or -1 if one threw (the message is in the out buffer). */
__attribute__((export_name("jb_jobs")))
int32_t jb_jobs(void) {
    int count = 0;
    for (;;) {
        JSContext *c;
        int r = JS_ExecutePendingJob(rt, &c);
        if (r == 0) break;
        if (r < 0) { take_exception(); return -1; }
        count++;
        if (count > 1000000) break;
    }
    return count;
}

/* Memory use, for the host's report. */
__attribute__((export_name("jb_memory_used")))
uint64_t jb_memory_used(void) {
    JSMemoryUsage u;
    memset(&u, 0, sizeof u);
    JS_ComputeMemoryUsage(rt, &u);
    return (uint64_t)u.malloc_size;
}
