    /* Ends a `ServerConfig`: the current `SSL_CTX` (and, through ex_data, its
       ALPN list) is released once the last `SSL` made from it has been freed;
       OpenSSL counts the references. */
    kelp_tls_slot* slot = (kelp_tls_slot*)kelp_ctx;
    SSL_CTX_free(slot->ctx);
    pthread_mutex_destroy(&slot->mu);
    free(slot);
