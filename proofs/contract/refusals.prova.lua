-- The refusal names a product's planes say are the contract's, word for word (CONTRACT.md §8; K m/13614): every
-- name `anemnez_extension::peer` declares appears in CONTRACT.md. Read from the repository, so the crate's own tests
-- stay self-contained (a released crate has no docs/ beside it).

local PEER = "crates/anemnez-extension/src/peer.rs"
local CONTRACT = "docs/specs/CONTRACT.md"

prova.test("every refusal name the peer module declares is the contract's, word for word", {
	proves = "control-is-fabricd-only and the unreadable-code refusal are named in CONTRACT.md exactly as the "
		.. "planes say them, so a caller reading the contract matches what it is told",
}, function(t)
	local source = fs.read(prova.root .. "/" .. PEER)
	local contract = fs.read(prova.root .. "/" .. CONTRACT)
	local names = 0
	for name in source:gmatch('pub const [%u_]+: &str = "([^"]+)";') do
		names = names + 1
		t:expect(contract:find(name, 1, true) ~= nil, CONTRACT .. " never names " .. name):is_true()
	end
	-- Vacuity guard: no names found means the pattern broke, and this row asked nothing.
	t:expect(names, "no refusal names found in " .. PEER):gte(2)
end)
