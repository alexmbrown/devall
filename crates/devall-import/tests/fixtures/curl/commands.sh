# Copied from Chrome dev tools ("Copy as cURL (bash)")
curl 'https://api.acme.test/v1/users?page=2' \
  -H 'accept: application/json' \
  -H 'authorization: Bearer abc.def.ghi' \
  --compressed

curl -X POST https://api.acme.test/v1/users \
  -H "Content-Type: application/json" \
  -d '{"name": "Grace",
"roles": ["admin"]}'

curl -u admin:pa55word -F 'file=@./report.pdf;type=application/pdf' -F note=Q3 https://api.acme.test/v1/upload
