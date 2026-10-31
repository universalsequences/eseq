;; eseq.modal.materials — materials for eseq.modal objects, as plain data.
;;
;; A material is a dict; an object is made of one, or offers a menu of
;; several:
;;
;;   (import eseq.modal :as m)
;;   (import eseq.modal.materials :as mat)
;;   (m/string 'str :made-of mat/nylon)
;;   (m/plate 'body :made-of (list mat/spruce mat/maple mat/aluminium))
;;
;; Your own materials are more data: (mat/material :name "gut" ...) or a
;; tweaked copy, (merge mat/steel :name "spring steel" :young 2.1e11).
;;
;; Values are typical handbook figures (density kg/m^3, Young's modulus Pa,
;; Poisson's ratio); woods are along the grain. The first nine follow the
;; Modalys material table that the original keyword materials came from.

(module eseq.modal.materials)

(export material material?
        uranium steel brass aluminium granite nylon carbon diamond spruce
        copper bronze titanium silver gold lead glass ice maple rosewood mahogany
        cardboard rubber alder mylar water spider-silk)

;; A material.
;;   name     what the panel's material menu shows
;;   density  kg/m^3
;;   young    Young's modulus, Pa; with density it sets how stiff the object
;;            is for its mass, which stretches a string's upper partials
;;            and sets a plate's pitch
;;   poisson  Poisson's ratio; shapes a plate's pitch (strings ignore it)
;;   damping  its loss factor (eta), for a finite-element solid: each mode
;;            loses pi * eta * f per second; nil (most metals) for none
;;   wood     a wood's other constants, for a finite-element solid given a
;;            :grain: (dict :young-r :young-t :shear-lr :shear-lt :shear-rt
;;            :poisson-lr :poisson-lt :poisson-rt), Pa and ratios, with
;;            young the modulus along the grain (L), R radial and T
;;            tangential to the growth rings; nil for an isotropic material
(def material (&key (name "material") (density 1000) (young 1e9) (poisson 0.3) (wood nil) (damping nil))
  (let ((m (dict :kind :material :name name :density density :young young :poisson poisson))
        (with-wood (if wood (merge m :wood wood) m)))
    (if damping (merge with-wood :damping damping) with-wood)))

;; A wood's constants from the Wood Handbook's ratios to its modulus along
;; the grain (E_R/E_L, E_T/E_L, G_LR/E_L, G_LT/E_L, G_RT/E_L) and its
;; Poisson ratios.
(def wood-ratios (young r t lr lt rt nu-lr nu-lt nu-rt)
  (dict :young-r (* young r) :young-t (* young t) :shear-lr (* young lr) :shear-lt (* young lt)
        :shear-rt (* young rt) :poisson-lr nu-lr :poisson-lt nu-lt :poisson-rt nu-rt))

;; True when x is a material.
;;   x  anything
(def material? (x) (if (map? x) (= (get x :kind) :material) false))

(def uranium   (material :name "uranium"   :density 19100 :young 2.08e11 :poisson 0.23))
(def steel     (material :name "steel"     :density 7850  :young 2.0e11  :poisson 0.29))
(def brass     (material :name "brass"     :density 8500  :young 1.0e11  :poisson 0.34))
(def aluminium (material :name "aluminium" :density 2700  :young 7.0e10  :poisson 0.33))
(def granite   (material :name "granite"   :density 2700  :young 4.0e10  :poisson 0.25))
(def nylon     (material :name "nylon"     :density 1140  :young 3.0e9   :poisson 0.4))
(def carbon    (material :name "carbon"    :density 1400  :young 1.1e12  :poisson 0.3))
(def diamond   (material :name "diamond"   :density 3500  :young 1.22e12 :poisson 0.2))
;; Sitka spruce: soundboards
(def spruce    (material :name "spruce"    :density 450   :young 1.1e10  :poisson 0.37
                         :wood (wood-ratios 1.1e10 0.078 0.043 0.064 0.061 0.003 0.372 0.467 0.435)
                         :damping 0.008))
(def copper    (material :name "copper"    :density 8960  :young 1.17e11 :poisson 0.34))
;; bell bronze (about 80% copper, 20% tin)
(def bronze    (material :name "bronze"    :density 8600  :young 1.05e11 :poisson 0.34))
(def titanium  (material :name "titanium"  :density 4500  :young 1.16e11 :poisson 0.32))
(def silver    (material :name "silver"    :density 10490 :young 8.3e10  :poisson 0.37))
(def gold      (material :name "gold"      :density 19300 :young 7.9e10  :poisson 0.42))
(def lead      (material :name "lead"      :density 11340 :young 1.6e10  :poisson 0.44))
;; soda-lime glass
(def glass     (material :name "glass"     :density 2500  :young 7.0e10  :poisson 0.22))
(def ice       (material :name "ice"       :density 917   :young 9.0e9   :poisson 0.33))
;; paperboard (a tissue box, a toilet-paper roll): light, soft and lossy, so a
;; cardboard solid thuds rather than rings
;; polyester film, a drum head's skin
(def mylar     (material :name "mylar"     :density 1390  :young 4.0e9   :poisson 0.38))
;; not a solid at all: its bulk modulus stands in for Young's, so a "water"
;; head or string is about as stiff for its weight as nylon (Poisson's ratio
;; near the incompressible 0.5); a what-if material, as Modalys allows
(def water     (material :name "water"     :density 1000  :young 2.2e9   :poisson 0.49))
;; spider dragline silk: light, and for its weight about a third as stiff as
;; steel, three times nylon
(def spider-silk (material :name "spider_silk" :density 1300 :young 1.0e10 :poisson 0.3))
(def cardboard (material :name "cardboard" :density 700   :young 3.0e9   :poisson 0.3
                         :damping 0.04))
;; a rubber band: so soft for its weight that a string of it barely stretches
;; its partials
(def rubber    (material :name "rubber"    :density 1100  :young 5.0e6   :poisson 0.49))
;; Woods are along the grain; their :damping is the loss factor along it
;; (spruce, prized for soundboards, rings longest); their :wood constants (Wood Handbook ratios:
;; sugar maple, Honduras mahogany; rosewood, which the handbook lacks, as a
;; generic dense hardwood) make a solid with :grain orthotropic.
(def maple     (material :name "maple"     :density 650   :young 1.2e10  :poisson 0.35
                         :wood (wood-ratios 1.2e10 0.132 0.065 0.111 0.063 0.021 0.424 0.476 0.774)
                         :damping 0.012))
(def rosewood  (material :name "rosewood"  :density 850   :young 1.6e10  :poisson 0.35
                         :wood (wood-ratios 1.6e10 0.12 0.07 0.08 0.06 0.02 0.40 0.50 0.65)
                         :damping 0.01))
;; red alder: Fender solid bodies (a Precision Bass, a Stratocaster). The
;; handbook gives its density and modulus but no elastic ratios; these are
;; yellow-poplar's, the closest listed diffuse-porous hardwood of its weight
(def alder     (material :name "alder"     :density 450   :young 9.5e9   :poisson 0.35
                         :wood (wood-ratios 9.5e9 0.092 0.043 0.075 0.069 0.011 0.318 0.392 0.703)
                         :damping 0.01))
;; Honduras mahogany: guitar necks and bodies
(def mahogany  (material :name "mahogany"  :density 560   :young 1.05e10 :poisson 0.3
                         :wood (wood-ratios 1.05e10 0.107 0.064 0.066 0.086 0.028 0.314 0.533 0.600)
                         :damping 0.012))
