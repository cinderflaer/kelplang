    /* The server's `SSL_CTX`, from PEM bytes `std/tls` read through the `fs`
       gate. `kelp_tls_server_ctx` (the OpenSSL-only helper in
       `tls_helpers.c`) does the work and reports a failure with a fixed
       sentence that never quotes the key. */
    char text[160];
    int code = 0;
    SSL_CTX* ctx = kelp_tls_server_ctx(
        kelp_cert->data, kelp_cert->len,
        kelp_key->data, kelp_key->len,
        kelp_ca->data, kelp_ca->len,
        kelp_alpn->data, kelp_alpn->len,
        kelp_min13 != 0, &code, text, sizeof text);
    *kelp_err_code = code;
    if (ctx == NULL) {
        free(kelp_reason->data);
        *kelp_reason = kelp_String__from_str(text);
    }
    return ctx;
