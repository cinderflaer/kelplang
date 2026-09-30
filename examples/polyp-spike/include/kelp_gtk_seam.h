#include <gtk/gtk.h>

/* GTK's own `g_signal_connect` is a macro over `g_signal_connect_data` that
   casts its handler to `GCallback` and its destroy notify to
   `GClosureNotify`. A generated FFI call cannot spell either cast, so this
   seam carries them: it takes the `(thunk, env, destroy)` triple a
   `move closure(Widget)` parameter expands into, and nothing else.

   `void (*handler)(void*, void*)` is the thunk of a one-argument closure: the
   generated thunk always appends its `void* user_data`, and a `Widget`
   parameter is a `void*` typedef. That signature is coupled to the closure
   being one handle argument wide. */
static inline gulong kelp_gtk_connect_clicked(GtkWidget* instance,
                                             void (*handler)(void*, void*),
                                             void* data,
                                             void (*destroy)(void*)) {
    return g_signal_connect_data(instance, "clicked", (GCallback)handler, data,
                                 (GClosureNotify)destroy, 0);
}
