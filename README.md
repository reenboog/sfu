This is a Rust-based Selective Forwarding Unit (SFU) Server built on top of MediaSoup, offering scalable, real-time video/audio relay infrastructure. It supports advanced features like simulcast, temporal/spatial scalability (SVC), and dynamic track management — with a custom signaling layer optimized for low-latency, multi-user sessions.

Highlights

-Simulcast & SVC: full support for multi-layered video encoding and bandwidth-adaptive forwarding.
-Dynamic Bitrate & Layer Switching: clients can request resolution/quality changes on the fly via lightweight API calls.
-Multi-Room Support: concurrent call rooms with isolated track routing and user state.
-Custom Signaling Layer: purpose-built signaling protocol (not SDP-based) for secure, flexible media negotiation and routing.

Architecture
-Media transport via MediaSoup Workers
-User sessions and authentication
-Track creation and relabeling
-API-based layer negotiation (POST /set-layer, etc.)
-Broadcast and peer-to-peer routing logic

Use Cases
-Secure group video/audio calls with scalable relay
-Custom conferencing tools with dynamic media control
-Developer-facing media backends for apps (e.g., chat, education, games)

flow:

/join?userId={USER_ID}
/join?userId={USER_ID}&roomId={ROOM_ID}
