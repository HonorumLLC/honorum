// Shared schedules and seat alerts link to the root with their target in the
// fragment, which the server never sees, so it may answer with the landing
// page. Those links belong to the app.
if (location.pathname === '/' && location.hash.length > 1) location.replace('/app' + location.hash);
