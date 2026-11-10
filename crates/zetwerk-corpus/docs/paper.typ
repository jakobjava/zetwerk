#set page(paper: "a4", margin: 2.5cm, numbering: "1")
#set text(size: 11pt, lang: "en")
#set heading(numbering: "1.1")
#set math.equation(numbering: "(1)")

#align(center)[
  #text(size: 17pt, weight: "bold")[On the Convergence of Iterated Averages]

  A. Author #h(2em) B. Author

  _Institute for Applied Examples_
]

*Abstract.* We study a simple averaging scheme and show that it converges
at a geometric rate under mild assumptions. The argument is elementary and
relies only on a contraction estimate, which we state in @sec-main and prove
in @sec-proof.

= Introduction <sec-intro>

Averaging schemes appear throughout numerical analysis. Given a sequence of
points, one replaces each point by a weighted mean of its neighbours and
repeats the process. Although each step is trivial, the long-term behaviour
is not obvious, and a careful statement requires some notation.

Let $x_0, x_1, dots, x_n$ be real numbers and fix a weight $lambda$ with
$0 < lambda < 1$. We define the averaged sequence by

$ y_i = lambda x_i + (1 - lambda) x_(i+1) $ <eq-avg>

for $i = 0, dots, n - 1$. Applying @eq-avg repeatedly produces a triangular
array whose final entry is a convex combination of the original points.

= Main result <sec-main>

Our main result is the following estimate.

/ Theorem: Let $d_k$ denote the diameter of the $k$-th row of the array.
  Then $d_(k+1) <= max(lambda, 1 - lambda) dot d_k$ for every $k$.

In particular the diameters decay geometrically:

$ d_k <= rho^k d_0, quad rho = max(lambda, 1 - lambda). $ <eq-rate>

A few remarks are in order.

- The constant $rho$ in @eq-rate is sharp when $n = 1$.
- For $lambda = 1/2$ the rate is $1/2$, independent of the data.
- The estimate does *not* depend on the dimension of the ambient space.

= Proof <sec-proof>

Fix two indices $i < j$ in row $k + 1$. By definition,

$ y_j - y_i = lambda (x_j - x_i) + (1 - lambda)(x_(j+1) - x_(i+1)), $

and each difference on the right is bounded by $d_k$ in absolute value.
The triangle inequality therefore gives $|y_j - y_i| <= d_k$, which is not
yet enough. The improvement comes from observing that consecutive averages
share an endpoint#footnote[This is the only place where the structure of the
scheme is used.], so that one of the two terms can be absorbed.

+ Write the difference as a telescoping sum.
+ Bound each summand using the shared endpoint.
+ Sum the resulting geometric series.

Carrying out these steps yields the claim. #h(1fr) $square$

= Conclusion

We have shown that iterated averaging contracts at a geometric rate. The
proof extends without change to normed spaces, as noted in @sec-main.
