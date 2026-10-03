    /* The protocol the handshake settled on, or `false` when none was
       negotiated (no ALPN offered, or none configured on the other side). */
    const unsigned char* data = NULL;
    unsigned int len = 0;
    SSL_get0_alpn_selected((SSL*)kelp_handle, &data, &len);
    if (data == NULL || len == 0) {
        return false;
    }
    char* text = (char*)malloc((size_t)len + 1);
    if (text == NULL) {
        kelp_panic("tls.alpn: out of memory");
    }
    memcpy(text, data, len);
    text[len] = 0;
    free(kelp_out->data);
    *kelp_out = kelp_String__from_str(text);
    free(text);
    return true;
