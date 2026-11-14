/* Options contract only: never calls sentry_init or installs a crash handler. */
#include <sentry.h>
#include <string.h>

int main(void) {
    sentry_options_t *options = sentry_options_new();
    if (!options) return 1;
    sentry_options_set_dsn(options, "");
    sentry_options_set_require_user_consent(options, 1);
    sentry_options_set_auto_session_tracking(options, 0);
    sentry_options_set_debug(options, 0);
    sentry_options_set_max_breadcrumbs(options, 0);
    sentry_options_set_proxy(options, "");
    sentry_options_set_cache_keep(options, 0);
    const char *dsn = sentry_options_get_dsn(options);
    const char *proxy = sentry_options_get_proxy(options);
    int refused = (dsn && strcmp(dsn, "") != 0)
        || !sentry_options_get_require_user_consent(options)
        || sentry_options_get_auto_session_tracking(options)
        || sentry_options_get_debug(options)
        || sentry_options_get_max_breadcrumbs(options) != 0
        || !proxy || strcmp(proxy, "") != 0
        || sentry_options_get_cache_keep(options) != 0;
    sentry_options_free(options);
    return refused ? 1 : 0;
}
