    /* Replaces the context a `ServerConfig` hands to new handshakes. The new
       context is built first; if that fails the old one stays in place and the
       failure is reported, so a bad renewal never takes the server down.
       Sessions already made keep the old context alive through OpenSSL's
       reference count. */
    char text[160];
    int code = 0;
    SSL_CTX* fresh = kelp_tls_server_ctx(
        kelp_cert->data, kelp_cert->len,
        kelp_key->data, kelp_key->len,
        kelp_ca->data, kelp_ca->len,
        kelp_alpn->data, kelp_alpn->len,
        kelp_pass->data, kelp_pass->len,
        kelp_min13 != 0, &code, text, sizeof text);
    *kelp_err_code = code;
    if (fresh == NULL) {
        free(kelp_reason->data);
        *kelp_reason = kelp_String__from_str(text);
        return;
    }
    kelp_tls_slot* slot = (kelp_tls_slot*)kelp_ctx;
    pthread_mutex_lock(&slot->mu);
    SSL_CTX* old = slot->ctx;
    slot->ctx = fresh;
    pthread_mutex_unlock(&slot->mu);
    SSL_CTX_free(old);
