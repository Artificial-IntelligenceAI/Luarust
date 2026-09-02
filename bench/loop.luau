-- Luau: Roblox's Lua, with gradual types and its own native code generator. Every number
-- is a double, as in Lua 5.1 -- and every value this loop holds stays under 2^53, so each
-- one is held exactly and the annotation is honest.
local sum: number = 0
for i = 1, ITERATIONS do
    sum = (sum + i) % 1000000007
end
print(sum)
