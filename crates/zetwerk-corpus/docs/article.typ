#set page(paper: "a4", margin: (x: 3cm, y: 2.5cm), numbering: "1")
#set text(size: 10pt, lang: "en")
#set par(justify: true, first-line-indent: 1em, spacing: 0.65em)
#set heading(numbering: "1.")
#set math.equation(numbering: "(1)")

= Spectral estimates

Let $A$ be a self-adjoint operator on a Hilbert space $H$ with compact
resolvent, and denote its eigenvalues by $mu_1 <= mu_2 <= dots$, repeated
according to multiplicity. The counting function $N(mu)$ records the number
of eigenvalues not exceeding $mu$, and its growth encodes a surprising amount
of geometric information about the underlying problem.

The classical result in this direction is the Weyl law. For the Dirichlet
Laplacian on a bounded domain $Omega subset RR^d$ it states that

$ N(mu) = (omega_d |Omega|) / (2 pi)^d mu^(d slash 2) + o(mu^(d slash 2)), quad mu -> oo, $ <eq-weyl>

where $omega_d$ is the volume of the unit ball. The leading term depends on
the domain only through its volume, which is the precise sense in which one
cannot hear the shape of a drum from the first-order asymptotics alone.

Refinements of @eq-weyl require control of the remainder. Under a
non-periodicity condition on the billiard flow one obtains a second term
proportional to the surface measure of the boundary,

$ N(mu) = c_d |Omega| mu^(d slash 2) - c'_d |partial Omega| mu^((d-1) slash 2) + o(mu^((d-1) slash 2)), $

with explicit dimensional constants $c_d$ and $c'_d$. The proof proceeds by
a Tauberian argument applied to the trace of the wave group, and the
condition on the flow enters through the singularities of that trace at
non-zero times.

= Variational characterisation

The eigenvalues admit the min-max description

$ mu_k = min_(V subset D(A) \ dim V = k) max_(u in V \ u != 0) (chevron.l A u, u chevron.r) / (norm(u)^2), $ <eq-minmax>

which makes monotonicity under domain inclusion immediate. If
$Omega_1 subset Omega_2$ then every trial space for the smaller domain is
admissible for the larger one, hence $mu_k (Omega_2) <= mu_k (Omega_1)$ for
all $k$. Combined with explicit computations on cubes this already yields
two-sided bounds of the correct order, a technique usually called
Dirichlet--Neumann bracketing.

For Schrödinger operators $-Delta + V$ with a confining potential the same
strategy applies after localisation. One covers space by small cubes on
which $V$ is nearly constant, applies @eq-minmax on each cube, and sums the
contributions. The resulting phase-space volume formula reads

$ N(mu) tilde (2 pi)^(-d) |{(x, xi) : |xi|^2 + V(x) <= mu}|, $

and it remains valid under rather weak regularity assumptions on $V$. The
error terms, however, are sensitive to the behaviour of the potential near
its minima, where the semiclassical approximation breaks down and a direct
analysis of the harmonic oscillator becomes necessary.

= Heat kernel methods

An alternative route passes through the heat trace
$Z(t) = sum_k e^(-t mu_k)$. Its small-time expansion,

$ Z(t) tilde (4 pi t)^(-d slash 2) sum_(j >= 0) a_j t^(j slash 2), quad t -> 0^+, $

has coefficients $a_j$ given by integrals of local geometric invariants:
$a_0$ is the volume, $a_1$ is a multiple of the boundary area, and $a_2$
involves the total scalar and mean curvatures. Karamata's Tauberian theorem
converts the leading behaviour of $Z$ into @eq-weyl, but it cannot recover
lower-order terms, which is why the wave equation method is needed for
sharper results.
