fetch('/__stage3__/blocked-fetch.txt')
  .then(response => postMessage({
    kind: response.status === 204 ? 'request-blocked' : 'request-allowed',
    status: response.status
  }))
  .catch(error => postMessage({ kind: 'request-failed', detail: String(error) }));
