sum = 0
for i in range(1, ITERATIONS_PLUS_ONE):
    sum = (sum + i) % 1000000007
print(sum)
