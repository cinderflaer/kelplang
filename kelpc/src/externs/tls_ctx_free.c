    /* Ends a `ServerConfig`: the `SSL_CTX` (and, through ex_data, its ALPN
       list) is released once the last `SSL` made from it has been freed;
       OpenSSL counts the references. */
    SSL_CTX_free((SSL_CTX*)kelp_ctx);
