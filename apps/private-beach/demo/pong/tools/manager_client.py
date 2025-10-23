from __future__ import annotations

import json
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from typing import Dict, Generator, Iterable, Optional, Tuple


@dataclass
class ControllerLease:
    controller_token: str
    expires_at_ms: int


class ManagerRequestError(RuntimeError):
    pass


class PrivateBeachManagerClient:
    def __init__(self, base_url: str, token: Optional[str], timeout: float = 10.0):
        self.base_url = base_url.rstrip("/")
        self.token = token
        self.timeout = timeout

    # --------------------------------------------------------------------- HTTP
    def _request(
        self,
        method: str,
        path: str,
        body: Optional[object] = None,
        acceptable: Tuple[int, ...] = (200, 201, 202, 204),
    ) -> Optional[object]:
        url = urllib.parse.urljoin(self.base_url + "/", path.lstrip("/"))
        headers = {"Accept": "application/json"}
        data: Optional[bytes] = None
        if body is not None:
            data = json.dumps(body, separators=(",", ":")).encode("utf-8")
            headers["Content-Type"] = "application/json"
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        request = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(request) as response:
                if response.status not in acceptable:
                    payload = response.read().decode("utf-8", errors="ignore")
                    raise ManagerRequestError(
                        f"{method} {url} returned {response.status}: {payload}"
                    )
                if response.length == 0:
                    return None
                raw = response.read()
                if not raw:
                    return None
                return json.loads(raw.decode("utf-8"))
        except urllib.error.HTTPError as exc:  # pragma: no cover - network failures
            detail = exc.read().decode("utf-8", errors="ignore")
            raise ManagerRequestError(
                f"{method} {url} failed ({exc.code}): {detail}"
            ) from exc
        except urllib.error.URLError as exc:  # pragma: no cover - network failures
            raise ManagerRequestError(
                f"{method} {url} transport error: {exc.reason}"
            ) from exc

    # -------------------------------------------------------------- Session Ops
    def attach_session(self, private_beach_id: str, session_id: str) -> None:
        payload = {"origin_session_ids": [session_id]}
        self._request(
            "POST",
            f"/private-beaches/{private_beach_id}/sessions/attach",
            payload,
        )

    def update_session_metadata(
        self,
        session_id: str,
        metadata: Dict[str, str],
        location_hint: Optional[str] = None,
    ) -> None:
        payload: Dict[str, object] = {"metadata": metadata}
        if location_hint:
            payload["location_hint"] = location_hint
        self._request("PATCH", f"/sessions/{session_id}", payload)

    def list_sessions(self, private_beach_id: str) -> Iterable[Dict[str, object]]:
        result = self._request(
            "GET", f"/private-beaches/{private_beach_id}/sessions", None
        )
        return result or []

    # ----------------------------------------------------------- Controller Ops
    def create_controller_pairing(
        self,
        controller_session_id: str,
        child_session_id: str,
        prompt_template: Optional[str] = None,
        update_cadence: Optional[str] = None,
    ) -> Dict[str, object]:
        payload: Dict[str, object] = {"child_session_id": child_session_id}
        if prompt_template is not None:
            payload["prompt_template"] = prompt_template
        if update_cadence is not None:
            payload["update_cadence"] = update_cadence
        result = self._request(
            "POST",
            f"/sessions/{controller_session_id}/controllers",
            payload,
        )
        if not isinstance(result, dict):
            raise ManagerRequestError("unexpected pairing response payload")
        return result

    def acquire_controller_lease(
        self,
        controller_session_id: str,
        ttl_ms: Optional[int] = None,
        reason: Optional[str] = None,
    ) -> ControllerLease:
        payload: Dict[str, object] = {}
        if ttl_ms is not None:
            payload["ttl_ms"] = ttl_ms
        if reason:
            payload["reason"] = reason
        result = self._request(
            "POST",
            f"/sessions/{controller_session_id}/controller/lease",
            payload,
        )
        if not isinstance(result, dict):
            raise ManagerRequestError("unexpected controller lease response")
        token = result.get("controller_token")
        expires = result.get("expires_at_ms")
        if not isinstance(token, str) or not isinstance(expires, int):
            raise ManagerRequestError("controller lease missing token or expiry")
        return ControllerLease(controller_token=token, expires_at_ms=expires)

    # --------------------------------------------------------------- Action Ops
    def queue_terminal_write(
        self, session_id: str, controller_token: str, command: Dict[str, object]
    ) -> None:
        payload = {"controller_token": controller_token, "actions": [command]}
        self._request("POST", f"/sessions/{session_id}/actions", payload)

    def list_controller_pairings(
        self, controller_session_id: str
    ) -> Iterable[Dict[str, object]]:
        result = self._request(
            "GET", f"/sessions/{controller_session_id}/controllers", None
        )
        return result or []

    # --------------------------------------------------------------- State Feed
    def subscribe_state(
        self,
        session_id: str,
    ) -> Generator[Dict[str, object], None, None]:
        url = urllib.parse.urljoin(
            self.base_url + "/", f"/sessions/{session_id}/state/stream"
        )
        headers = {"Accept": "text/event-stream"}
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        request = urllib.request.Request(url, headers=headers, method="GET")
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                buffer = []
                for raw_line in response:
                    line = raw_line.decode("utf-8", errors="ignore").rstrip("\r\n")
                    if line.startswith("data:"):
                        buffer.append(line[5:].strip())
                    elif not line:
                        if not buffer:
                            continue
                        data_str = "\n".join(buffer)
                        buffer.clear()
                        if not data_str:
                            continue
                        try:
                            payload = json.loads(data_str)
                        except json.JSONDecodeError:
                            continue
                        yield payload
        except urllib.error.URLError as exc:  # pragma: no cover - network failures
            raise ManagerRequestError(
                f"state subscription failed for {session_id}: {exc.reason}"
            ) from exc
