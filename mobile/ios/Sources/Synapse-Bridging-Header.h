#pragma once

#include <stddef.h>

/// Start embedded web asset host; returns malloc'd load URL (caller must free).
char *synapse_web_url(void);
