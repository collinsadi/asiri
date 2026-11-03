# Reads `ss -Htlnp` and prints the public TCP services other than SSH (22) and LiveKit's media
# port (7881), one "port/tcp (process)" per line: what a firewall that admits only those two
# would cut off. Used by install.sh before it changes anything; tested by check.sh.
{
  addr = $4
  port = addr; sub(/.*:/, "", port)
  host = addr; sub(/:[^:]*$/, "", host)
  if (host ~ /^127\./ || host == "[::1]" || host ~ /%lo$/) next
  if (port == 22 || port == 7881 || (port in seen)) next
  seen[port] = 1
  name = "unknown"
  if (match($0, /users:\(\("[^"]+"/)) name = substr($0, RSTART + 9, RLENGTH - 10)
  printf "%s/tcp (%s)\n", port, name
}
