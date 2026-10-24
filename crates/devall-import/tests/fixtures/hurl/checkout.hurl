# Log in and keep the token.
POST https://shop.example.org/api/login
Content-Type: application/json
{
  "user": "bob",
  "password": "{{password}}"
}
HTTP 200
Content-Type: application/json
[Captures]
token: jsonpath "$.token"
csrf: header "X-CSRF-Token"
[Asserts]
jsonpath "$.status" == "ok"
jsonpath "$.roles" count == 2
duration < 1000

# Add to cart (form).
POST https://shop.example.org/api/cart
Authorization: Bearer {{token}}
[FormParams]
sku: A-42
quantity: 2
[Options]
location: true
HTTP 302

# Upload a receipt.
POST https://shop.example.org/api/receipts
[MultipartFormData]
note: thanks
receipt: file,receipt.pdf; application/pdf
[BasicAuth]
bob: {{password}}
HTTP *

# Search.
GET https://shop.example.org/api/search
[QueryStringParams]
q: running shoes
page: 1
[Cookies]
theme: dark
HTTP 200
[Asserts]
header "Content-Type" contains "json"
jsonpath "$.items[0].name" exists
