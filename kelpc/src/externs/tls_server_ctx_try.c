    /* The server's configuration slot, from PEM bytes `std/tls` read through the
       `fs` gate. `kelp_tls_server_ctx` (the OpenSSL-only helper in
       `tls_helpers.c`) builds the `SSL_CTX` and reports a failure with a fixed
       sentence that never quotes the key or the pass phrase. The returned
       handle is a slot that holds the current context, so `reload_try_raw` can
       replace it while connections are being accepted. */
    char text[160];
    int code = 0;
    SSL_CTX* ctx = kelp_tls_server_ctx(
        kelp_cert->data, kelp_cert->len,
        kelp_key->data, kelp_key->len,
        kelp_ca->data, kelp_ca->len,
        kelp_alpn->data, kelp_alpn->len,
        kelp_pass->data, kelp_pass->len,
        kelp_min13 != 0, &code, text, sizeof text);
    *kelp_err_code = code;
    if (ctx == NULL) {
        free(kelp_reason->data);
        *kelp_reason = kelp_String__from_str(text);
        return NULL;
    }
    kelp_tls_slot* slot = (kelp_tls_slot*)malloc(sizeof *slot);
    if (slot == NULL) {
        SSL_CTX_free(ctx);
        *kelp_err_code = 5;
        free(kelp_reason->data);
        *kelp_reason = kelp_String__from_str("out of memory");
        return NULL;
    }
    slot->ctx = ctx;
    pthread_mutex_init(&slot->mu, NULL);
    return slot;
