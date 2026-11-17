-- Dated ends in the serving face's code are held to their dates (D m/15228, E m/15226): an interim that fails open,
-- like principal::Unpublished admitting every tokenless call, is never open-ended. Each `SUNSET: Owner <lane>; ...
-- Ends YYYY-MM-DD` in the files below is red by name once today (UTC) is past its date, so a product still on it is
-- named in the gate, not only in its own Health line. Moving a date is the owner's word, in a reviewed change.

local FILES = { "crates/anemnez-extension-serve/src/principal.rs" }

prova.test("every dated SUNSET in the serving face is before its end", {
	switch = "quality",
	proves = "an interim that fails open carries an owner and an end, and the gate turns red past the end",
}, function(t)
	local today = os.date("!%Y-%m-%d")
	local found = 0
	for _, file in ipairs(FILES) do
		local source = fs.read(prova.root .. "/" .. file)
		for owner, ends in source:gmatch("SUNSET: Owner (%u);.-Ends (%d%d%d%d%-%d%d%-%d%d)") do
			found = found + 1
			t:expect(today <= ends, file .. ": the SUNSET owned by " .. owner .. " ended " .. ends .. " (today " .. today .. ")")
				:is_true()
		end
	end
	-- Vacuity guard: no dated SUNSET found means the pattern broke, and this row asked nothing.
	t:expect(found, "no dated SUNSET found in " .. table.concat(FILES, ", ")):gte(1)
end)
